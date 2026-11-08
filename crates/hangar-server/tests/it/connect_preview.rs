//! Prévia de porta: o listener de verdade entre um visitante e um projeto falso em socket cru, para
//! conferir o byte que passou. Programas e "internet" são falsos.
use crate::connect_port::{device, free_port};
use crate::connect_supervisor::{alone, gate, linked, pid_of, read_pid, until};
use crate::fake::{Fake, OWNER, client, config, spawn_fake, spawn_state};

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::http::header;
use axum::routing::get;
use hangar_server::connect::config::{render_frpc, render_preview_site};
use hangar_server::connect::preview::{COOKIE, LINK_LIFE, LINK_PARAM, Previews, SESSION_LIFE, router};
use hangar_server::connect::state::{self, Linked};
use hangar_server::connect::supervisor::Supervisor;
use hangar_server::routes::AppState;
use hangar_server::share::owner_routes::Reach;
use hangar_server::share_reach::Probe;
use reqwest::header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION, SET_COOKIE};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

const HOST: &str = "pc1--preview.acme.hangar.dev.br";
const SITE: &str = "reverse_proxy 127.0.0.1:8769";
const PATIENCE: Duration = Duration::from_secs(10);

async fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte).await {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

struct Project {
    port: u16,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Project {
    fn hits(&self) -> usize { self.heads.lock().unwrap().len() }

    /// O último pedido como o projeto o leu, em minúsculas: nome de cabeçalho não tem caixa.
    fn last(&self) -> String { self.heads.lock().unwrap().last().unwrap().to_ascii_lowercase() }
}

async fn answer(mut stream: TcpStream, port: u16, seen: Arc<Mutex<Vec<String>>>) {
    let head = read_head(&mut stream).await;
    if head.is_empty() {
        return;
    }
    seen.lock().unwrap().push(head.clone());
    if head.to_ascii_lowercase().contains("upgrade: websocket") {
        stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: x\r\n\r\n").await.unwrap();
        let mut buffer = [0u8; 64];
        while let Ok(read) = stream.read(&mut buffer).await {
            if read == 0 || stream.write_all(&buffer[..read]).await.is_err() {
                break;
            }
        }
        return;
    }
    let redirect = |target: String| format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let reply = if head.starts_with("GET /away ") {
        redirect(format!("http://127.0.0.1:{port}/in"))
    } else if head.starts_with("GET /elsewhere ") {
        redirect("http://127.0.0.1:8765/api/sessions".to_owned())
    } else if head.starts_with("GET /stream ") {
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n").await.unwrap();
        std::future::pending::<()>().await;
        return;
    } else if head.starts_with("GET /hang ") {
        std::future::pending::<()>().await;
        return;
    } else if head.starts_with("GET /cookies ") {
        "HTTP/1.1 200 OK\r\nClear-Site-Data: \"cookies\"\r\nSet-Cookie: __Host-hangar_preview=; Max-Age=0\r\nSet-Cookie: cp_token=x\r\nSet-Cookie: app=1; Path=/\r\nSet-Cookie: shared=1; Domain=hangar.dev.br; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned()
    } else {
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned()
    };
    let _ = stream.write_all(reply.as_bytes()).await;
}

async fn project() -> Project {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let heads: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = heads.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(answer(stream, port, seen.clone()));
        }
    });
    Project { port, heads }
}

struct Site {
    addr: SocketAddr,
    previews: Arc<Previews>,
    now: Arc<AtomicU64>,
}

async fn site() -> Site {
    let now = Arc::new(AtomicU64::new(1_000));
    let clock = now.clone();
    let previews = Arc::new(Previews::new().with_clock(Arc::new(move || clock.load(Ordering::SeqCst))));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(previews.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Site { addr, previews, now }
}

impl Site {
    /// O Caddy repassa o `Host` do site.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        client().get(format!("http://{}{path}", self.addr)).header("host", HOST)
    }

    fn with_link(&self, link: &str) -> reqwest::RequestBuilder {
        self.get(&format!("/?{LINK_PARAM}={link}"))
    }

    fn open(&self, project: &Project) -> String {
        self.previews.open(project.port, HOST).unwrap()
    }

    /// O botão da página, que manda a `Origin` do próprio site.
    fn press_at(&self, path: &str) -> reqwest::RequestBuilder {
        client().post(format!("http://{}{path}", self.addr)).header("host", HOST).header("origin", format!("https://{HOST}"))
    }

    fn press(&self, link: &str) -> reqwest::RequestBuilder {
        self.press_at(&format!("/?{LINK_PARAM}={link}"))
    }

    /// Abre a página, aperta o botão e devolve o cookie como o navegador o manda de volta.
    async fn enter(&self, link: &str) -> String {
        assert_eq!(self.with_link(link).send().await.unwrap().status(), 200);
        let response = self.press(link).send().await.unwrap();
        assert_eq!(response.status(), 303);
        response.headers()[SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned()
    }

    async fn inside(&self, project: &Project) -> String {
        let link = self.open(project);
        self.enter(&link).await
    }
}

/// Pedido escrito à mão, para o que o cliente HTTP não deixa mandar.
async fn raw(addr: SocketAddr, request: &str) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let head = read_head(&mut stream).await;
    (stream, head)
}

fn upgrade(path: &str, extra: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {HOST}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{extra}\r\n")
}

#[tokio::test]
async fn preview_refuses_a_visitor_without_the_link() {
    let (site, project) = (site().await, project().await);
    assert_eq!(site.get("/").send().await.unwrap().status(), 401, "sem prévia aberta");

    let link = site.open(&project);
    assert_eq!(site.get("/").send().await.unwrap().status(), 401);
    assert_eq!(site.with_link("errado").send().await.unwrap().status(), 401);
    assert_eq!(site.get("/").header("cookie", format!("{COOKIE}={link}")).send().await.unwrap().status(), 401, "o link não vale como sessão");
    assert_eq!(client().get(format!("http://{}/?{LINK_PARAM}={link}", site.addr)).send().await.unwrap().status(), 421, "outro host não gasta o link");
    let head = client().head(format!("http://{}/?{LINK_PARAM}={link}", site.addr)).header("host", HOST);
    assert_eq!(head.send().await.unwrap().status(), 401);
    let bare = client().post(format!("http://{}/?{LINK_PARAM}={link}", site.addr)).header("host", HOST);
    assert_eq!(bare.send().await.unwrap().status(), 401, "sem a `Origin` do site o link não é gasto");
    let neighbour = client().post(format!("http://{}/?{LINK_PARAM}={link}", site.addr)).header("host", HOST)
        .header("origin", "https://outra--preview.acme.hangar.dev.br");
    assert_eq!(neighbour.send().await.unwrap().status(), 401);

    let cookie = site.enter(&link).await;
    let other_site = client().get(format!("http://{}/", site.addr)).header("cookie", &cookie).header("host", "pc1.acme.hangar.dev.br");
    assert_eq!(other_site.send().await.unwrap().status(), 421, "o cookie não vale noutro host");
    let doubled = format!("GET / HTTP/1.1\r\nHost: {HOST}\r\nHost: evil.example\r\nCookie: {cookie}\r\n\r\n");
    let (_stream, head) = raw(site.addr, &doubled).await;
    assert!(head.starts_with("HTTP/1.1 421"), "{head}");
    assert_eq!(project.hits(), 0);
}

#[tokio::test]
async fn preview_link_sets_a_host_cookie_and_drops_the_query() {
    let (site, project) = (site().await, project().await);
    let link = site.open(&project);
    let response = site.press_at(&format!("/app/page?x=1&{LINK_PARAM}={link}&y=2")).send().await.unwrap();
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()[LOCATION], "/app/page?x=1&y=2");
    assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    let cookie = response.headers()[SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with(&format!("{COOKIE}=")), "{cookie}");
    assert!(cookie.ends_with(&format!("; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_LIFE}")), "{cookie}");
    assert!(!cookie.contains(&link), "o cookie não é o link");
    assert_eq!(project.hits(), 0, "o link nunca chega ao projeto");

    let link = site.open(&project);
    let response = site.press_at(&format!("//evil.example/?{LINK_PARAM}={link}")).send().await.unwrap();
    assert_eq!(response.headers()[LOCATION], "/evil.example/", "a 303 não sai do site");
}

#[tokio::test]
async fn preview_page_shows_a_button_and_spends_nothing() {
    let (site, project) = (site().await, project().await);
    let link = site.open(&project);
    for _ in 0..2 {
        let response = site.with_link(&link).send().await.unwrap();
        assert_eq!(response.status(), 200, "buscar o link para a miniatura não o queima");
        let headers = response.headers().clone();
        assert_eq!(headers[CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        // `no-referrer` faria o botão sair com `Origin: null`.
        assert_eq!(headers["referrer-policy"], "same-origin");
        assert!(!headers.contains_key(SET_COOKIE));
        let page = response.text().await.unwrap();
        assert!(page.contains(r#"<form method="post">"#) && !page.contains("<script"), "{page}");
        assert!(!page.contains(&link), "a página não repete o link");
    }
    assert_eq!(site.press(&link).send().await.unwrap().status(), 303);
    assert_eq!(project.hits(), 0);
}

#[tokio::test]
async fn preview_keeps_the_project_away_from_the_site_cookies() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    let response = site.get("/cookies").header("cookie", &cookie).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("clear-site-data"));
    let cookies: Vec<&str> = response.headers().get_all(SET_COOKIE).iter().map(|value| value.to_str().unwrap()).collect();
    assert_eq!(cookies, ["app=1; Path=/", "shared=1; Path=/"], "o projeto guarda os dele, só neste host");
}

#[tokio::test]
async fn preview_refuses_a_service_worker_script() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    let script = site.get("/sw.js").header("cookie", &cookie).header("service-worker", "script");
    assert_eq!(script.send().await.unwrap().status(), 403);
    assert_eq!(project.hits(), 0);
}

#[tokio::test]
async fn preview_refuses_a_neighbour_without_origin() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    let fetched = |fetch_site: &str, mode: &str, dest: &str| site.get("/").header("cookie", &cookie)
        .header("sec-fetch-site", fetch_site).header("sec-fetch-mode", mode).header("sec-fetch-dest", dest);
    assert_eq!(fetched("same-site", "no-cors", "image").send().await.unwrap().status(), 403, "`<img>` de uma vizinha");
    assert_eq!(fetched("cross-site", "navigate", "iframe").send().await.unwrap().status(), 403, "moldura não é abrir o site");
    assert_eq!(project.hits(), 0);
    for (fetch_site, mode, dest) in [("same-site", "navigate", "document"), ("cross-site", "navigate", "document"),
        ("same-origin", "cors", "empty"), ("none", "navigate", "document")] {
        assert_eq!(fetched(fetch_site, mode, dest).send().await.unwrap().status(), 200, "{fetch_site} {mode} {dest}");
    }
}

#[tokio::test]
async fn preview_session_ends_at_its_deadline() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    site.now.fetch_add(SESSION_LIFE - 1, Ordering::SeqCst);
    assert!(site.previews.state().is_some_and(|open| open.entered));
    let (mut stream, head) = raw(site.addr, &upgrade("/hmr", &format!("Cookie: {cookie}\r\n"))).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    let mut rest = [0u8; 1];
    let ended = timeout(PATIENCE, stream.read(&mut rest)).await.unwrap();
    assert!(matches!(ended, Ok(0) | Err(_)), "{ended:?}");

    site.now.fetch_add(1, Ordering::SeqCst);
    assert_eq!(site.get("/").header("cookie", &cookie).send().await.unwrap().status(), 401);
    assert_eq!(site.previews.state(), None, "vencida não aparece como aberta");
}

#[tokio::test]
async fn preview_does_not_wait_for_a_project_that_never_answers() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    let waiting = tokio::spawn(site.get("/hang").header("cookie", &cookie).send());
    until("o pedido chegar ao projeto", || project.hits() == 1).await;
    site.previews.close();
    let response = timeout(PATIENCE, waiting).await.unwrap().unwrap().unwrap();
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn preview_proxies_http_and_websocket() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;

    let response = site.get("/page?token=vite").header("cookie", format!("{cookie}; app=1; cp_token=senha"))
        .bearer_auth("credencial").header("origin", format!("https://{HOST}"))
        .header("referer", format!("https://{HOST}/?{LINK_PARAM}=gasto")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "ok");
    let seen = project.last();
    assert!(seen.starts_with("get /page?token=vite http/1.1"), "{seen}");
    assert!(seen.contains(&format!("host: 127.0.0.1:{}", project.port)), "{seen}");
    assert!(seen.contains(&format!("origin: http://127.0.0.1:{}", project.port)), "{seen}");
    assert!(seen.contains("cookie: app=1\r\n"), "{seen}");
    for hidden in ["authorization", "credencial", "hangar_preview", "cp_token", "senha", HOST] {
        assert!(!seen.contains(hidden), "{hidden}: {seen}");
    }

    let extra = format!("Cookie: {cookie}\r\nOrigin: https://{HOST}\r\n");
    let (mut stream, head) = raw(site.addr, &upgrade("/hmr?token=vite", &extra)).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    stream.write_all(b"ping").await.unwrap();
    let mut echoed = [0u8; 4];
    timeout(PATIENCE, stream.read_exact(&mut echoed)).await.unwrap().unwrap();
    assert_eq!(&echoed, b"ping");
    let seen = project.last();
    assert!(seen.starts_with("get /hmr?token=vite http/1.1"), "{seen}");
    assert!(!seen.contains("hangar_preview"), "{seen}");

    // O DELETE encerra também o que já estava passando.
    site.previews.close();
    let mut rest = [0u8; 1];
    let ended = timeout(PATIENCE, stream.read(&mut rest)).await.unwrap();
    assert!(matches!(ended, Ok(0) | Err(_)), "{ended:?}");
}

#[tokio::test]
async fn preview_needs_the_session_and_this_origin() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    let (_stream, head) = raw(site.addr, &upgrade("/hmr", "")).await;
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");
    // A vizinha é o mesmo site para o navegador: o cookie vem junto.
    let neighbour = "https://outra--preview.acme.hangar.dev.br";
    let (_stream, head) = raw(site.addr, &upgrade("/hmr", &format!("Cookie: {cookie}\r\nOrigin: {neighbour}\r\n"))).await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    let post = client().post(format!("http://{}/save", site.addr)).header("host", HOST).header("cookie", &cookie).header("origin", neighbour);
    assert_eq!(post.send().await.unwrap().status(), 403);
    assert_eq!(project.hits(), 0);
}

#[tokio::test]
async fn preview_link_is_single_use_and_expires() {
    let (site, project) = (site().await, project().await);
    let link = site.open(&project);
    let cookie = site.enter(&link).await;
    assert_eq!(site.with_link(&link).send().await.unwrap().status(), 401, "segundo uso");
    assert_eq!(site.press(&link).send().await.unwrap().status(), 401, "segundo uso");
    assert_eq!(site.get("/").header("cookie", &cookie).send().await.unwrap().status(), 200);
    let again = site.with_link(&link).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(again.status(), 303, "com a sessão, o link velho só sai da URL");
    assert_eq!(again.headers()[LOCATION], "/");
    assert!(!again.headers().contains_key(SET_COOKIE));

    let late = site.open(&project);
    site.now.fetch_add(LINK_LIFE, Ordering::SeqCst);
    assert_eq!(site.with_link(&late).send().await.unwrap().status(), 401, "vencido");
    assert_eq!(site.press(&late).send().await.unwrap().status(), 401, "vencido");

    assert_eq!(site.get("/").header("cookie", &cookie).send().await.unwrap().status(), 401, "prévia nova derruba a sessão da velha");
    let cookie = site.inside(&project).await;
    site.previews.close();
    assert_eq!(site.get("/").header("cookie", &cookie).send().await.unwrap().status(), 401, "fechada, o cookie repetido não vale");
}

#[tokio::test]
async fn preview_cuts_a_long_response_when_it_closes() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    let mut response = site.get("/stream").header("cookie", &cookie).send().await.unwrap();
    assert_eq!(&response.chunk().await.unwrap().unwrap()[..], b"hello");
    site.previews.close();
    let ended = timeout(PATIENCE, response.chunk()).await.unwrap();
    assert!(!matches!(ended, Ok(Some(_))), "{ended:?}");
}

#[tokio::test]
async fn preview_never_follows_a_redirect_from_the_project() {
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    let own = site.get("/away").header("cookie", &cookie).send().await.unwrap();
    assert_eq!(own.status(), 302);
    assert_eq!(own.headers()[LOCATION], "/in", "o endereço local vira caminho do site");
    let other = site.get("/elsewhere").header("cookie", &cookie).send().await.unwrap();
    assert_eq!(other.status(), 302);
    assert_eq!(other.headers()[LOCATION], "http://127.0.0.1:8765/api/sessions");
    assert_eq!(project.hits(), 2, "o destino nunca é seguido daqui");
}

struct Server {
    _tmp: tempfile::TempDir,
    _python: Arc<Fake>,
    dir: PathBuf,
    supervisor: Arc<Supervisor>,
    main: SocketAddr,
    connect: SocketAddr,
    preview: SocketAddr,
    upstream: SocketAddr,
    device: String,
}

async fn wait_open(addr: SocketAddr) {
    timeout(PATIENCE, async {
        while TcpStream::connect(addr).await.is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
}

/// O DNS público responde 127.0.0.1 e a "internet" é a própria porta da prévia, ou o nome ainda não existe.
async fn reach(port: u16, found: bool) -> Reach {
    let answer = if found { r#"{"Status":0,"Answer":[{"name":"x.","type":1,"TTL":60,"data":"127.0.0.1"}]}"# } else { r#"{"Status":3}"# };
    let app = Router::new().route("/resolve", get(move || async move { ([(header::CONTENT_TYPE, "application/dns-json")], answer) }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let doh = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Reach {
        probe: Probe { doh: vec![format!("http://{doh}/resolve")], funnel_scheme: "http", funnel_port: 1, connect_port: port, local_base: String::new() },
        every: Duration::from_millis(20),
        deadline: Duration::from_millis(if found { 5_000 } else { 200 }),
    }
}

async fn server() -> Server {
    server_with(true).await
}

/// Servidor de verdade ligado ao Connect, com o `frpc` falso entrando na hora.
async fn server_with(found: bool) -> Server {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let supervisor = Arc::new(alone(&dir));
    state::write(&dir, &linked()).unwrap();
    std::fs::write(gate(&dir), "").unwrap();
    let (python, upstream) = spawn_fake().await;
    let mut app = AppState::new(config(upstream, "127.0.0.1"));
    let (connect, preview) = (free_port(), free_port());
    app.connect = Some(connect);
    app.preview = Some(preview);
    app.connect_supervisor = supervisor.clone();
    app.connect_preview_reach = Arc::new(reach(preview.port(), found).await);
    let device = device(&mut app);
    let main = spawn_state(app).await;
    wait_open(connect).await;
    wait_open(preview).await;
    Server { _tmp: tmp, _python: python, dir, supervisor, main, connect, preview, upstream, device }
}

fn open_request(addr: SocketAddr, token: &str, port: u16) -> reqwest::RequestBuilder {
    client().post(format!("http://{addr}/api/connect/preview")).bearer_auth(token).body(format!(r#"{{"port":{port}}}"#))
}

async fn replied(request: reqwest::RequestBuilder) -> (u16, Value) {
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test]
async fn preview_never_reaches_hangar_ports() {
    let s = server().await;
    for port in [8765, 8766, 8768, 8769, 18080, 18443, 8317, 0, s.main.port(), s.connect.port(), s.preview.port(), s.upstream.port()] {
        let (status, body) = replied(open_request(s.main, OWNER, port)).await;
        assert_eq!(status, 422, "{port}: {body}");
        assert_eq!(body["detail"]["code"], "connect_preview_port_refused");
    }
    let (status, body) = replied(open_request(s.connect, &s.device, 8765)).await;
    assert_eq!(status, 422, "aparelho vale como dono e cai na mesma recusa: {body}");
    assert!(!state::read(&s.dir).unwrap().unwrap().preview_published, "recusa não gasta certificado");

    // A porta é conferida a cada pedido, e o destino nunca sai do pedido.
    let (site, project) = (site().await, project().await);
    let cookie = site.inside(&project).await;
    for request in [
        format!("GET http://127.0.0.1:8765/api/sessions HTTP/1.1\r\nHost: {HOST}\r\nCookie: {cookie}\r\n\r\n"),
        format!("CONNECT 127.0.0.1:8765 HTTP/1.1\r\nHost: {HOST}\r\nCookie: {cookie}\r\n\r\n"),
    ] {
        let (_stream, head) = raw(site.addr, &request).await;
        assert!(head.starts_with("HTTP/1.1 400"), "{request}: {head}");
    }
    site.previews.reserve([project.port]);
    assert_eq!(site.get("/").header("cookie", &cookie).send().await.unwrap().status(), 422);
    assert_eq!(project.hits(), 0);
}

#[tokio::test]
async fn preview_site_is_born_on_first_use_and_stays() {
    let s = server().await;
    let (status, _) = replied(client().post(format!("http://{}/api/connect/preview", s.main)).body(r#"{"port":3000}"#)).await;
    assert_eq!(status, 401);
    assert!(!state::read(&s.dir).unwrap().unwrap().preview_published);

    let (status, body) = replied(open_request(s.main, OWNER, 3000)).await;
    assert_eq!(status, 200, "{body}");
    let url = body["url"].as_str().unwrap();
    assert!(url.starts_with(&format!("https://{HOST}/?{LINK_PARAM}=")), "{url}");
    assert!(state::read(&s.dir).unwrap().unwrap().preview_published);
    let frpc =std::fs::read_to_string(s.dir.join("frpc.toml")).unwrap();
    assert!(frpc.contains(&format!(r#"customDomains = ["{HOST}"]"#)), "{frpc}");
    until("Caddy de pé", || read_pid(&s.dir, "caddy").is_some()).await;
    assert!(std::fs::read_to_string(s.dir.join("Caddyfile")).unwrap().contains(SITE));
    until("Connect de pé", || s.supervisor.status().enabled).await;

    let state = || client().get(format!("http://{}/api/connect/preview", s.main)).bearer_auth(OWNER);
    let (status, body) = replied(state()).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["preview"]["port"], 3000);
    assert_eq!(body["preview"]["entered"], false);

    // Trocar a porta do projeto não toca nos filhos: o site aponta sempre para a 8769.
    let (frpc_pid, caddy_pid) = (pid_of(&s.dir, "frpc"), pid_of(&s.dir, "caddy"));
    let (status, body) = replied(open_request(s.connect, &s.device, 3001)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!((pid_of(&s.dir, "frpc"), pid_of(&s.dir, "caddy")), (frpc_pid, caddy_pid));

    let close = |addr: SocketAddr, token: &str| client().delete(format!("http://{addr}/api/connect/preview")).bearer_auth(token.to_owned());
    assert_eq!(replied(close(s.connect, &s.device)).await.0, 200);
    assert_eq!(replied(close(s.main, "errado")).await.0, 401);
    assert_eq!(replied(state()).await.1["preview"], Value::Null);

    // Publicado, mas com os filhos parados: o link não abriria para ninguém.
    s.supervisor.stop().await;
    let (status, body) = replied(open_request(s.main, OWNER, 3000)).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["detail"]["code"], "connect_not_running");
    assert_eq!(replied(state()).await.1["preview"], Value::Null);
    s.supervisor.stop().await;
}

#[tokio::test]
async fn preview_link_waits_for_the_first_site_to_open_from_outside() {
    let s = server_with(false).await;
    let (status, body) = replied(open_request(s.main, OWNER, 3000)).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["detail"]["code"], "connect_preview_reach_failed");
    assert!(state::read(&s.dir).unwrap().unwrap().preview_published, "o site fica: refazer gastaria outro certificado");

    // Publicado não quer dizer alcançável: a segunda tentativa confere de novo.
    until("Connect de pé", || s.supervisor.status().enabled).await;
    let (status, body) = replied(open_request(s.main, OWNER, 3000)).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["detail"]["code"], "connect_preview_reach_failed");

    // O DNS segue sem o nome: depois de responder uma vez, nada confere de novo.
    s.supervisor.preview_answered();
    let (status, body) = replied(open_request(s.main, OWNER, 3000)).await;
    assert_eq!(status, 200, "só espera até a primeira resposta: {body}");
    s.supervisor.stop().await;
}

#[tokio::test]
async fn a_preview_port_held_elsewhere_says_why() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let supervisor = alone(&dir);
    state::write(&dir, &linked()).unwrap();
    supervisor.preview_port_failed("endereço em uso".into());
    let refused = supervisor.publish_preview().await.unwrap_err();
    assert_eq!(refused.code, "connect_preview_port_closed");
    assert!(refused.msg.contains("endereço em uso"), "{}", refused.msg);
    supervisor.preview_port_opened();
    supervisor.preview_port_failed("de novo".into());
    assert!(supervisor.publish_preview().await.unwrap_err().msg.contains("de novo"));
}

#[tokio::test]
async fn preview_site_waits_for_this_server_to_open_the_port() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let supervisor = alone(&dir);
    state::write(&dir, &linked()).unwrap();
    std::fs::write(gate(&dir), "").unwrap();

    let refused = supervisor.publish_preview().await.unwrap_err();
    assert_eq!((refused.status.as_u16(), refused.code), (409, "connect_preview_port_closed"));
    assert!(!state::read(&dir).unwrap().unwrap().preview_published);

    // Volta do servidor com o site publicado e a 8769 na mão de outro: a entrada do dono sobe sozinha.
    state::write(&dir, &Linked { preview_published: true, ..linked() }).unwrap();
    supervisor.resume().await;
    until("Caddy de pé", || read_pid(&dir, "caddy").is_some()).await;
    assert!(!std::fs::read_to_string(dir.join("Caddyfile")).unwrap().contains(SITE));

    let (frpc_pid, caddy_pid) = (pid_of(&dir, "frpc"), pid_of(&dir, "caddy"));
    supervisor.preview_port_opened();
    until("Caddy novo", || read_pid(&dir, "caddy").is_some_and(|pid| pid != caddy_pid)).await;
    assert!(std::fs::read_to_string(dir.join("Caddyfile")).unwrap().contains(SITE));
    assert_eq!(pid_of(&dir, "frpc"), frpc_pid, "o `frpc` já tinha o proxy");
    supervisor.stop().await;
}

#[test]
fn preview_proxy_exists_only_once_published() {
    let mut value = linked();
    assert!(!render_frpc(&value).unwrap().contains("--preview"));
    value.preview_published = true;
    let text = render_frpc(&value).unwrap();
    assert_eq!(text.matches("[[proxies]]").count(), 2, "{text}");
    assert!(render_preview_site(&value).contains(SITE));

    // `connect.json` de antes do campo vale como nunca publicado.
    let mut old = serde_json::to_value(linked()).unwrap();
    old.as_object_mut().unwrap().remove("preview_published");
    assert!(!serde_json::from_value::<Linked>(old).unwrap().preview_published);
}
