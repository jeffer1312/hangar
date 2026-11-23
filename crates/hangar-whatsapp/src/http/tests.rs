//! As rotas contra um C2 de mentira em loopback, que confere a assinatura do pedido como o binário e
//! assina (ou não) a resposta.
use std::sync::Mutex;

use axum::body::Body;
use hangar_core::auth::{Auth, TrustedHosts};
use hangar_core::diag::DiagClient;
use hangar_core::guest::GuestGrant;
use tower::ServiceExt;

use super::*;
use crate::control;

const OWNER: &str = "dono-whatsapp";
const KEY: [u8; 32] = [7; 32];
/// Cabe num `i32` e passa do maior pid do Linux: nunca é um processo vivo.
const DEAD_PID: u32 = 2_147_483_000;

struct Scene {
    home: tempfile::TempDir,
    wa: Arc<WhatsApp>,
    /// Pedidos que o C2 de mentira recebeu, como `METHOD URI`.
    seen: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Signing {
    Signed,
    Unsigned,
    /// O primeiro pedido leva um 307 sem assinatura para o mesmo endereço; o seguinte é atendido assinado.
    Redirect,
}

impl Scene {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let wa = WhatsApp::with_context(Context::new(home.path()));
        Self { home, wa, seen: Arc::default() }
    }

    fn paths(&self) -> &install::Paths {
        &self.wa.ctx.paths
    }

    fn write_control(&self, port: u16, pid: u32) {
        std::fs::create_dir_all(&self.paths().data).unwrap();
        let key: String = KEY.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(self.paths().data.join("control.key"), format!("{key}\n")).unwrap();
        std::fs::write(self.paths().data.join("control.json"), json!({"version": 1, "api": 1, "port": port, "pid": pid}).to_string()).unwrap();
    }

    /// Sobe o C2 de mentira e aponta o `control.json` para ele, com o pid deste processo (vivo).
    async fn serve(&self, signing: Signing) {
        let seen = self.seen.clone();
        let app = Router::new().fallback(move |request: Request| {
            let seen = seen.clone();
            async move { fake_c2(request, signing, &seen).await }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        self.write_control(port, std::process::id());
    }

    async fn call(&self, method: Method, uri: &str, owner: bool, body: Option<&str>) -> (u16, Value) {
        self.send(method, uri, owner, None, body).await
    }

    async fn send(&self, method: Method, uri: &str, owner: bool, guest: Option<Caller>, body: Option<&str>) -> (u16, Value) {
        let edge = Edge { trusted: TrustedHosts::parse("127.0.0.1"), auth: Auth::new(OWNER), diag: DiagClient::new() };
        let app: Router = router(edge, self.wa.clone());
        let mut request = axum::http::Request::builder().method(method).uri(uri);
        if owner {
            request = request.header("authorization", format!("Bearer {OWNER}"));
        }
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let mut request = request.body(body.map_or_else(Body::empty, |b| Body::from(b.to_owned()))).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
        if let Some(guest) = guest {
            request.extensions_mut().insert(guest);
        }
        let response = app.oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let raw = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
    }
}

/// Confere o pedido como o `control.VerifyRequest` do binário e responde como as rotas do C2.
async fn fake_c2(request: Request, signing: Signing, seen: &Mutex<Vec<String>>) -> Response {
    let method = request.method().as_str().to_owned();
    let uri = request.uri().path_and_query().map_or("/", |p| p.as_str()).to_owned();
    let header = request.headers().get(control::SIGNATURE_HEADER).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    let body = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let first = {
        let mut seen = seen.lock().unwrap();
        seen.push(format!("{method} {uri}"));
        seen.len() == 1
    };
    if signing == Signing::Redirect && first {
        return (StatusCode::TEMPORARY_REDIRECT, [("location", uri.as_str())]).into_response();
    }
    let (ts, _) = header.split_once('.').unwrap_or_default();
    let valid = ts.parse::<i64>().is_ok_and(|ts| control::sign_request(&KEY, &method, &uri, ts, &body) == header);
    let (status, out): (u16, Vec<u8>) = if !valid {
        (401, br#"{"detail":{"code":"unauthorized","msg":"assinatura"}}"#.to_vec())
    } else {
        match (method.as_str(), uri.split('?').next().unwrap_or_default()) {
            ("GET", "/v1/status") => (200, json!({"api": 1, "version": "0.1.0", "state": "pairing", "qr": "2@abc,def"}).to_string().into_bytes()),
            ("GET", "/v1/messages") => (200, json!({"chat": "5564999@s.whatsapp.net", "name": "Ana",
                "messages": [{"id": "A1", "at": "2026-11-22T10:00:00Z", "from_me": false, "kind": "text", "text": "oi"}], "more": false})
                .to_string().into_bytes()),
            ("POST", "/v1/pair") => (202, Vec::new()),
            ("POST", "/v1/logout") => (409, br#"{"detail":{"code":"not_paired","msg":"nenhum aparelho"}}"#.to_vec()),
            ("POST", "/v1/wipe") => (200, br#"{"logout":"done"}"#.to_vec()),
            _ => (404, br#"{"detail":{"code":"bad_request","msg":"rota desconhecida"}}"#.to_vec()),
        }
    };
    let mut response = (StatusCode::from_u16(status).unwrap(), out.clone()).into_response();
    if signing != Signing::Unsigned {
        let signature = control::sign_response(&KEY, ts, status, &out);
        response.headers_mut().insert(control::SIGNATURE_HEADER, signature.parse().unwrap());
    }
    response
}

fn code(body: &Value) -> &str {
    body["detail"]["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn a_guest_is_refused_and_a_stranger_needs_the_token() {
    let s = Scene::new();
    s.serve(Signing::Signed).await;
    let guest = Caller::Guest(Arc::new(GuestGrant {
        token: "convidado".into(), token_hash: String::new(), sessions: Default::default(), pair_roots: vec![], invite: true }));
    for uri in ["/api/whatsapp", "/api/whatsapp/messages?chat=5564999"] {
        let (status, body) = s.send(Method::GET, uri, true, Some(guest.clone()), None).await;
        assert_eq!((status, code(&body)), (403, "whatsapp_owner_only"), "{uri}");
    }
    let (status, body) = s.send(Method::POST, "/api/whatsapp/install", true, Some(guest), Some(r#"{"accept":true}"#)).await;
    assert_eq!((status, code(&body)), (403, "whatsapp_owner_only"));
    assert!(!s.paths().accepted().exists(), "convidado não aceita por ninguém");
    let (status, _) = s.call(Method::GET, "/api/whatsapp", false, None).await;
    assert_eq!(status, 401);
    assert!(s.seen.lock().unwrap().is_empty(), "nada chegou ao serviço");
}

#[tokio::test]
async fn install_without_the_acceptance_is_400() {
    let s = Scene::new();
    for body in [None, Some("{}"), Some(r#"{"accept":false}"#), Some(r#"{"accept":"sim"}"#)] {
        let (status, reply) = s.call(Method::POST, "/api/whatsapp/install", true, body).await;
        assert_eq!((status, code(&reply)), (400, "whatsapp_accept_required"), "{body:?}");
    }
    assert!(!s.paths().accepted().exists());
    assert_eq!(s.wa.installer.job(), Value::Null, "nenhuma tarefa começou");
}

#[tokio::test]
async fn reads_with_the_service_off_are_409() {
    let s = Scene::new();
    let off = |(status, body): (u16, Value)| {
        assert_eq!((status, code(&body)), (409, "whatsapp_off"));
        assert!(body["detail"]["msg"].as_str().unwrap().contains("Configurações > WhatsApp"));
    };
    off(s.call(Method::GET, "/api/whatsapp/chats", true, None).await);
    off(s.call(Method::POST, "/api/whatsapp/pair", true, None).await);
    s.write_control(1, DEAD_PID);
    off(s.call(Method::GET, "/api/whatsapp/messages?chat=5564999", true, None).await);
    off(s.call(Method::POST, "/api/whatsapp/media/request", true, Some(r#"{"chat":"5564999"}"#)).await);
    let (status, body) = s.call(Method::GET, "/api/whatsapp", true, None).await;
    assert_eq!(status, 200);
    assert_eq!((&body["installed"], &body["service"], &body["status"]), (&json!(false), &json!("absent"), &Value::Null));
}

#[tokio::test]
async fn an_unsigned_reply_is_502() {
    let s = Scene::new();
    s.serve(Signing::Unsigned).await;
    let (status, body) = s.call(Method::GET, "/api/whatsapp/messages?chat=5564999", true, None).await;
    assert_eq!((status, code(&body)), (502, "whatsapp_untrusted"));
    assert!(body.get("messages").is_none(), "o corpo sem assinatura não passa");
    let (status, body) = s.call(Method::GET, "/api/whatsapp", true, None).await;
    assert_eq!((status, &body["status"], &body["status_error"]), (200, &Value::Null, &json!("whatsapp_untrusted")));
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let s = Scene::new();
    s.serve(Signing::Redirect).await;
    let (status, body) = s.call(Method::GET, "/api/whatsapp/messages?chat=5564999", true, None).await;
    assert_eq!((status, code(&body)), (502, "whatsapp_untrusted"), "o 307 não tem assinatura");
    assert_eq!(s.seen.lock().unwrap().len(), 1, "o pedido assinado não foi reenviado ao Location");
}

/// `control.json` velho com um pid que voltou a existir (outro processo) e a porta fechada.
#[tokio::test]
async fn a_stale_control_with_a_live_pid_counts_as_stopped() {
    let s = Scene::new();
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    s.write_control(closed, std::process::id());
    let (status, body) = s.call(Method::GET, "/api/whatsapp", true, None).await;
    assert_eq!(status, 200);
    assert_eq!((&body["service"], &body["status"], &body["status_error"]), (&json!("absent"), &Value::Null, &Value::Null));
    std::fs::write(s.paths().data.join("messages.db"), b"x").unwrap();
    let (status, body) = s.call(Method::POST, "/api/whatsapp/wipe", true, Some(r#"{"confirm":true}"#)).await;
    assert_eq!((status, body), (200, json!({"logout": "not_done"})), "sem serviço de verdade, o Hangar apaga");
    assert!(!s.paths().data.join("messages.db").exists());
}

/// Só a saída 3 sem processo de pé vira o aviso de dados mais novos; o resto é o parado de sempre, também
/// quando o gerenciador não responde.
#[test]
fn a_service_that_quit_on_newer_data_says_so() {
    assert_eq!(stopped_error(Ok(ProcessState { running: false, last_exit: Some(3) })), json!("whatsapp_schema_newer"));
    assert_eq!(stopped_error(Ok(ProcessState { running: true, last_exit: Some(3) })), json!("whatsapp_schema_newer"), "unit antiga religando");
    assert_eq!(stopped_error(Ok(ProcessState { running: false, last_exit: Some(1) })), Value::Null);
    assert_eq!(stopped_error(Ok(ProcessState::default())), Value::Null);
    let unknown = ServiceError { step: "systemctl show".into(), detail: "Failed to connect to bus".into() };
    assert_eq!(stopped_error(Err(unknown)), Value::Null);
}

/// Apagar do disco exige o serviço confirmado parado: subindo é "subindo", sem resposta do gerenciador é
/// recusa própria, nunca "parado".
#[tokio::test]
async fn wipe_from_disk_needs_the_service_confirmed_stopped() {
    async fn refusal(running: Result<bool, ServiceError>) -> (u16, Value) {
        let response = wipe_guard(running).unwrap_err();
        let status = response.status().as_u16();
        let raw = axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap();
        (status, serde_json::from_slice(&raw).unwrap())
    }
    assert!(wipe_guard(Ok(false)).is_ok());
    let (status, body) = refusal(Ok(true)).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_service_starting"));
    let (status, body) = refusal(Err(ServiceError { step: "systemctl show".into(), detail: "Failed to connect to bus".into() })).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_service_state_unknown"), "{body}");
}

#[tokio::test]
async fn a_busy_install_does_not_record_the_acceptance() {
    let s = Scene::new();
    let _guard = s.wa.installer.inline().unwrap();
    let (status, body) = s.call(Method::POST, "/api/whatsapp/install", true, Some(r#"{"accept":true}"#)).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_busy"));
    assert!(!s.paths().accepted().exists());
}

#[tokio::test]
async fn a_wipe_during_an_update_waits_even_with_the_service_up() {
    let s = Scene::new();
    s.serve(Signing::Signed).await;
    let _guard = s.wa.installer.inline().unwrap();
    let (status, body) = s.call(Method::POST, "/api/whatsapp/wipe", true, Some(r#"{"confirm":true}"#)).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_busy"), "{body}");
    assert!(!s.seen.lock().unwrap().iter().any(|r| r.ends_with("/v1/wipe")), "o pedido não chegou ao serviço");
}

/// Um arquivo que não sai no meio (no Windows, aberto pelo serviço): a resposta diz o que já saiu.
#[cfg(unix)]
#[tokio::test]
async fn a_wipe_that_stops_midway_says_what_is_gone() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scene::new();
    let data = &s.paths().data;
    std::fs::create_dir_all(data.join("media")).unwrap();
    std::fs::write(data.join("messages.db"), b"x").unwrap();
    std::fs::write(data.join("media/a.jpg"), b"x").unwrap();
    std::fs::set_permissions(data.join("media"), std::fs::Permissions::from_mode(0o500)).unwrap();
    let (status, body) = s.call(Method::POST, "/api/whatsapp/wipe", true, Some(r#"{"confirm":true}"#)).await;
    std::fs::set_permissions(data.join("media"), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!((status, code(&body)), (500, "whatsapp_wipe_partial"), "{body}");
    assert_eq!(body["detail"]["params"], json!({"removed": ["messages.db"], "failed": "media"}));
}

#[tokio::test]
async fn messages_are_forwarded_signed_with_only_their_parameters() {
    let s = Scene::new();
    s.serve(Signing::Signed).await;
    let uri = format!("/api/whatsapp/messages?chat=5564999&limit=2&since=2026-11-01&before_id=&token={OWNER}&evil=1");
    let (status, body) = s.call(Method::GET, &uri, true, None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["messages"][0]["text"], "oi");
    assert_eq!(body["more"], false);
    assert_eq!(s.seen.lock().unwrap().as_slice(), ["GET /v1/messages?chat=5564999&limit=2&since=2026-11-01"],
        "sem o token, sem parâmetro estranho, sem vazio; a assinatura fechou com a query que foi no fio");
}

#[tokio::test]
async fn c2_errors_keep_the_status_with_the_house_code() {
    let s = Scene::new();
    s.serve(Signing::Signed).await;
    let (status, body) = s.call(Method::GET, "/api/whatsapp/search?q=oi", true, None).await;
    assert_eq!((status, code(&body)), (404, "whatsapp_bad_request"));
    let (status, body) = s.call(Method::POST, "/api/whatsapp/logout", true, None).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_not_paired"));
    let (status, body) = s.call(Method::POST, "/api/whatsapp/pair", true, Some(r#"{"phone":"5564999999999"}"#)).await;
    assert_eq!((status, body), (202, json!({})), "202 vazio vira {{}} para o cliente ler JSON");
}

#[tokio::test]
async fn the_overview_carries_the_status_and_the_qr() {
    let s = Scene::new();
    s.serve(Signing::Signed).await;
    let (status, body) = s.call(Method::GET, "/api/whatsapp", true, None).await;
    assert_eq!(status, 200);
    assert_eq!((&body["service"], &body["status"]["state"], &body["job"]), (&json!("running"), &json!("pairing"), &Value::Null));
    assert!(body["qr_svg"].as_str().unwrap().starts_with("<svg"), "{body}");
}

/// Binário que aceita a conexão e nunca responde: o resumo sai no prazo dele, antes do prazo dos clientes.
#[tokio::test]
async fn a_hung_service_makes_the_overview_unreachable_within_its_own_deadline() {
    let s = Scene::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    s.write_control(port, std::process::id());
    let started = std::time::Instant::now();
    let (status, body) = s.call(Method::GET, "/api/whatsapp", true, None).await;
    let elapsed = started.elapsed();
    assert_eq!(status, 200);
    assert_eq!((&body["service"], &body["status"], &body["status_error"]), (&json!("running"), &Value::Null, &json!("whatsapp_unreachable")));
    assert!((4..10).contains(&elapsed.as_secs()), "resumo levou {elapsed:?}");
}

#[tokio::test]
async fn wipe_goes_to_the_service_when_up_and_to_the_disk_when_stopped() {
    let s = Scene::new();
    let (status, body) = s.call(Method::POST, "/api/whatsapp/wipe", true, Some("{}")).await;
    assert_eq!((status, code(&body)), (400, "whatsapp_confirm_required"));
    s.serve(Signing::Signed).await;
    let (status, body) = s.call(Method::POST, "/api/whatsapp/wipe", true, Some(r#"{"confirm":true}"#)).await;
    assert_eq!((status, body), (200, json!({"logout": "done"})), "o corpo do C2 passa como veio");

    s.write_control(1, DEAD_PID);
    let data = &s.paths().data;
    for file in ["messages.db", "messages.db-wal", "session.db", "session.db-shm", "media/a.jpg"] {
        std::fs::create_dir_all(data.join(file).parent().unwrap()).unwrap();
        std::fs::write(data.join(file), b"x").unwrap();
    }
    let (status, body) = s.call(Method::POST, "/api/whatsapp/wipe", true, Some(r#"{"confirm":true}"#)).await;
    assert_eq!((status, body), (200, json!({"logout": "not_done"})));
    for gone in ["messages.db", "messages.db-wal", "session.db", "session.db-shm", "media"] {
        assert!(!data.join(gone).exists(), "{gone}");
    }
    assert!(data.join("control.key").exists(), "a chave fica para o binário subir igual");
}

#[tokio::test]
async fn uninstall_needs_confirmation_and_the_service_for_logout() {
    let s = Scene::new();
    let (status, body) = s.call(Method::POST, "/api/whatsapp/uninstall", true, Some(r#"{"logout":true}"#)).await;
    assert_eq!((status, code(&body)), (400, "whatsapp_confirm_required"));
    let (status, body) = s.call(Method::POST, "/api/whatsapp/uninstall", true, Some(r#"{"confirm":true,"logout":true}"#)).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_off"), "desvincular exige o serviço de pé");
}

#[test]
fn the_in_process_path_encodes_and_drops_empty_values() {
    assert_eq!(api_path("chats", &[]), "/api/whatsapp/chats");
    assert_eq!(api_path("search", &[("q", "bom dia&x"), ("chat", ""), ("limit", "5")]), "/api/whatsapp/search?q=bom+dia%26x&limit=5");
}

/// Contra o binário real. Rodar à mão:
/// `go build -o /tmp/wa/hangar-whatsapp ./cmd/hangar-whatsapp` no repo `hangar-whatsapp`,
/// `/tmp/wa/hangar-whatsapp --data /tmp/wa/data &`, e então
/// `HANGAR_WHATSAPP_INTEROP_DATA=/tmp/wa/data cargo test -p hangar-whatsapp -- --ignored interop`.
#[tokio::test]
#[ignore = "precisa do binário hangar-whatsapp rodando; ver o comentário"]
async fn interop_with_the_real_binary() {
    let data = std::env::var_os("HANGAR_WHATSAPP_INTEROP_DATA").expect("HANGAR_WHATSAPP_INTEROP_DATA");
    let client = Client::new(data.into());
    let status = client.status().await.expect("status assinado nos dois sentidos");
    assert_eq!(status["api"], 1);
    let chats = client.call(reqwest::Method::GET, "/v1/chats", &[("limit", "5"), ("q", "a b")], None).await.unwrap();
    assert_eq!(chats.status, 200);
    let missing = client.call(reqwest::Method::GET, "/v1/messages", &[], None).await.unwrap();
    assert_eq!(missing.status, 400, "erro também vem assinado");
}
