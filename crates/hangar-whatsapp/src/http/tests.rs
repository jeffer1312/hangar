//! As rotas contra um C2 de mentira em loopback, que confere a assinatura do pedido como o binário e
//! assina (ou não) a resposta.
use std::sync::Mutex;

use axum::body::Body;
use hangar_core::auth::{Auth, TrustedHosts};
use hangar_core::diag::DiagClient;
use hangar_core::guest::GuestGrant;
use tower::ServiceExt;

use super::*;
use crate::{control, outbox};

const OWNER: &str = "dono-whatsapp";
const KEY: [u8; 32] = [7; 32];
/// Cabe num `i32` e passa do maior pid do Linux: nunca é um processo vivo.
const DEAD_PID: u32 = 2_147_483_000;
/// A subida desta vida do Hangar na prova: o C5 falso cria os rascunhos às 10:00 do mesmo dia.
const STARTED: &str = "2026-11-24T09:00:00Z";

struct Scene {
    home: tempfile::TempDir,
    wa: Arc<WhatsApp>,
    /// Pedidos que o C2 de mentira recebeu, como `METHOD URI`.
    seen: Arc<Mutex<Vec<String>>>,
    /// Os rascunhos do C5 de mentira.
    drafts: Arc<Mutex<Vec<Value>>>,
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
        let started = chrono::DateTime::parse_from_rfc3339(STARTED).unwrap().with_timezone(&chrono::Utc);
        let wa = WhatsApp::with_outbox(Context::new(home.path()), outbox::Outbox::started_at(started));
        Self { home, wa, seen: Arc::default(), drafts: Arc::default() }
    }

    fn paths(&self) -> &install::Paths {
        &self.wa.ctx.paths
    }

    fn write_control(&self, port: u16, pid: u32) {
        self.write_control_api(port, pid, 1);
    }

    fn write_control_api(&self, port: u16, pid: u32, api: u32) {
        std::fs::create_dir_all(&self.paths().data).unwrap();
        let key: String = KEY.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(self.paths().data.join("control.key"), format!("{key}\n")).unwrap();
        std::fs::write(self.paths().data.join("control.json"), json!({"version": 1, "api": api, "port": port, "pid": pid}).to_string()).unwrap();
    }

    /// Sobe o C2 de mentira e aponta o `control.json` para ele, com o pid deste processo (vivo).
    async fn serve(&self, signing: Signing) {
        self.serve_api(signing, 1).await;
    }

    async fn serve_api(&self, signing: Signing, api: u32) {
        let (seen, drafts) = (self.seen.clone(), self.drafts.clone());
        let app = Router::new().fallback(move |request: Request| {
            let (seen, drafts) = (seen.clone(), drafts.clone());
            async move { fake_c2(request, signing, &seen, &drafts).await }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        self.write_control_api(port, std::process::id(), api);
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
async fn fake_c2(request: Request, signing: Signing, seen: &Mutex<Vec<String>>, drafts: &Mutex<Vec<Value>>) -> Response {
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
            (m, p) if p.starts_with("/v1/outbox") => fake_outbox(m, p, &uri, &body, drafts),
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

/// O C5 de mentira: rascunho pendente criado, lista dos pendentes, leitura, envio e recusa. `chat` `velho` é o
/// binário recusado pelo WhatsApp; `novo`, quem nunca escreveu para a conta.
fn fake_outbox(method: &str, path: &str, uri: &str, body: &[u8], drafts: &Mutex<Vec<Value>>) -> (u16, Vec<u8>) {
    let error = |status: u16, code: &str| (status, json!({"detail": {"code": code, "msg": format!("erro {code}")}}).to_string().into_bytes());
    let mut drafts = drafts.lock().unwrap();
    if path == "/v1/outbox" {
        return match method {
            "POST" => {
                let asked: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                match asked["chat"].as_str() {
                    Some("velho") => error(409, "outdated"),
                    Some("novo") => error(409, "new_contact"),
                    _ => {
                        let draft = json!({"id": format!("d{}", drafts.len() + 1), "origin": asked["origin"], "chat": asked["chat"],
                            "text": asked["text"], "state": "pending", "created_at": "2026-11-24T10:00:00Z", "expires_at": "2099-01-01T00:00:00Z",
                            "answered_at": null, "message_id": "", "error": "", "asked": asked});
                        drafts.push(draft.clone());
                        (201, draft.to_string().into_bytes())
                    }
                }
            }
            "GET" if uri.ends_with("?state=pending") => {
                let pending: Vec<&Value> = drafts.iter().filter(|d| d["state"] == "pending").collect();
                (200, json!({"drafts": pending}).to_string().into_bytes())
            }
            _ => error(400, "bad_request"),
        };
    }
    let rest = path.strip_prefix("/v1/outbox/").unwrap_or_default();
    let (id, action) = rest.split_once('/').unwrap_or((rest, ""));
    let Some(draft) = drafts.iter_mut().find(|d| d["id"] == id) else { return error(404, "not_found") };
    match (method, action) {
        ("GET", "") => (200, draft.to_string().into_bytes()),
        ("POST", "send" | "decline") if draft["state"] != "pending" => error(409, "answered"),
        ("POST", "send") => {
            draft["state"] = json!("sending");
            draft["answered_at"] = json!("2026-11-24T10:01:00Z");
            draft["via"] = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null)["via"].clone();
            (202, draft.to_string().into_bytes())
        }
        ("POST", "decline") if draft["fail_decline"] == true => error(500, "internal"),
        ("POST", "decline") => {
            draft["state"] = json!("declined");
            (200, draft.to_string().into_bytes())
        }
        _ => error(404, "bad_request"),
    }
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

/// Rascunho pela rota do dono: só os campos do C5 seguem, a resposta passa como veio e os erros do binário
/// ganham o prefixo `whatsapp_`; `outdated` diz para atualizar o binário.
#[tokio::test]
async fn drafts_are_created_and_answered_with_the_house_codes() {
    let s = Scene::new();
    s.serve_api(Signing::Signed, 2).await;
    let create = r#"{"chat":"5564999","text":"segue o zip","file":"/home/x/a.zip","origin":"casa::s0","origin_cwd":"/home/x","evil":1}"#;
    let (status, body) = s.call(Method::POST, "/api/whatsapp/outbox", true, Some(create)).await;
    assert_eq!((status, &body["id"], &body["state"]), (201, &json!("d1"), &json!("pending")), "{body}");
    assert_eq!(body["asked"], json!({"chat": "5564999", "text": "segue o zip", "file": "/home/x/a.zip", "origin": "casa::s0",
        "origin_cwd": "/home/x"}), "o campo estranho não chega ao binário");

    let (status, body) = s.call(Method::GET, "/api/whatsapp/outbox", true, None).await;
    assert_eq!((status, body["drafts"].as_array().map(Vec::len)), (200, Some(1)), "{body}");
    assert!(s.seen.lock().unwrap().iter().any(|r| r == "GET /v1/outbox?state=pending"), "sem state, os pendentes");
    let (status, body) = s.call(Method::GET, "/api/whatsapp/outbox/d1", true, None).await;
    assert_eq!((status, &body["state"]), (200, &json!("pending")));

    let (status, body) = s.call(Method::POST, "/api/whatsapp/outbox/d1/send", true, Some(r#"{"via":"pc"}"#)).await;
    assert_eq!((status, code(&body)), (400, "whatsapp_bad_request"));
    let (status, body) = s.call(Method::POST, "/api/whatsapp/outbox/d1/send", true, Some(r#"{"via":"mobile"}"#)).await;
    assert_eq!((status, &body["state"], &body["via"]), (202, &json!("sending"), &json!("mobile")), "{body}");
    let (status, body) = s.call(Method::POST, "/api/whatsapp/outbox/d1/decline", true, None).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_answered"));
    let (status, body) = s.call(Method::GET, "/api/whatsapp/outbox/d9", true, None).await;
    assert_eq!((status, code(&body)), (404, "whatsapp_not_found"));

    let (status, body) = s.call(Method::POST, "/api/whatsapp/outbox", true, Some(r#"{"chat":"velho","text":"oi","origin":"casa::s0"}"#)).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_outdated"));
    assert!(body["detail"]["msg"].as_str().unwrap().contains("atualize o hangar-whatsapp"), "{body}");
    let (status, body) = s.call(Method::POST, "/api/whatsapp/outbox", true, Some(r#"{"chat":"novo","text":"oi","origin":"casa::s0"}"#)).await;
    assert_eq!((status, code(&body)), (409, "whatsapp_new_contact"));
    let (status, body) = s.call(Method::POST, "/api/whatsapp/outbox", true, Some(r#"{"chat":5,"text":"oi","origin":"casa::s0"}"#)).await;
    assert_eq!((status, code(&body)), (400, "whatsapp_bad_request"));
    let (status, body) = s.call(Method::GET, "/api/whatsapp/outbox/a.b", true, None).await;
    assert_eq!((status, code(&body)), (400, "whatsapp_bad_request"), "id fora do formato não vai ao caminho do binário");
}

/// Binário da api 1 não tem saída: a recusa sai daqui, sem pedido ao serviço.
#[tokio::test]
async fn the_outbox_needs_the_send_api() {
    let s = Scene::new();
    s.serve(Signing::Signed).await;
    for (method, uri, body) in [(Method::POST, "/api/whatsapp/outbox", Some(r#"{"chat":"5564999","text":"oi","origin":"casa::s0"}"#)),
        (Method::GET, "/api/whatsapp/outbox", None), (Method::POST, "/api/whatsapp/outbox/d1/send", Some(r#"{"via":"desktop"}"#)),
        (Method::POST, "/api/whatsapp/outbox/d1/decline", None)] {
        let (status, reply) = s.call(method, uri, true, body).await;
        assert_eq!((status, code(&reply)), (409, "whatsapp_send_unsupported"), "{uri}");
    }
    assert!(s.seen.lock().unwrap().iter().all(|r| !r.contains("/v1/outbox")), "{:?}", s.seen.lock().unwrap());
}

#[tokio::test]
async fn a_guest_never_answers_a_draft() {
    let s = Scene::new();
    s.serve_api(Signing::Signed, 2).await;
    let guest = Caller::Guest(Arc::new(GuestGrant {
        token: "convidado".into(), token_hash: String::new(), sessions: Default::default(), pair_roots: vec![], invite: true }));
    for (method, uri) in [(Method::GET, "/api/whatsapp/outbox"), (Method::POST, "/api/whatsapp/outbox/d1/send")] {
        let (status, body) = s.send(method, uri, true, Some(guest.clone()), Some(r#"{"via":"desktop"}"#)).await;
        assert_eq!((status, code(&body)), (403, "whatsapp_owner_only"), "{uri}");
    }
    assert!(s.seen.lock().unwrap().is_empty());
}

#[derive(Default)]
struct Recorded {
    new: Mutex<Vec<String>>,
    failed: Mutex<Vec<&'static str>>,
}

impl outbox::Hooks for Recorded {
    fn new_draft(&self, draft: &Value) {
        self.new.lock().unwrap().push(draft["id"].as_str().unwrap_or_default().to_owned());
    }

    fn failed(&self, code: &'static str, _: &str) {
        self.failed.lock().unwrap().push(code);
    }
}

fn feed_ids(wa: &WhatsApp) -> Vec<String> {
    let feed = wa.outbox().feed().borrow().clone();
    feed.map(|f| f.drafts.iter().map(|d| d["id"].as_str().unwrap_or_default().to_owned()).collect()).unwrap_or_default()
}

fn pending_reads(s: &Scene) -> usize {
    s.seen.lock().unwrap().iter().filter(|r| *r == "GET /v1/outbox?state=pending").count()
}

/// Na primeira leitura desta vida, o pendente criado antes da subida é recusado e sai da lista sem relê-la. O
/// criado depois fica pelo horário, entra no evento com o `answer_by` e avisa uma vez só.
#[tokio::test]
async fn the_first_round_declines_what_another_life_left_pending() {
    let s = Scene::new();
    s.serve_api(Signing::Signed, 2).await;
    s.drafts.lock().unwrap().push(json!({"id": "d0", "state": "pending", "created_at": "2026-11-24T08:59:59Z",
        "expires_at": "2099-01-01T00:00:00Z"}));
    let (status, body) = s.call(Method::POST, "/api/whatsapp/outbox", true, Some(r#"{"chat":"5564999","text":"oi","origin":"casa::s0"}"#)).await;
    assert_eq!((status, &body["id"]), (201, &json!("d2")), "{body}");
    s.drafts.lock().unwrap().push(json!({"id": "d3", "state": "pending", "created_at": "2026-11-24T10:00:05Z",
        "expires_at": "2099-01-01T00:00:00Z"}));
    let hooks = Recorded::default();
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert_eq!(s.drafts.lock().unwrap()[0]["state"], json!("declined"), "o de outra vida saiu da fila");
    assert_eq!(s.drafts.lock().unwrap()[2]["state"], json!("pending"), "o criado nesta vida fica");
    assert_eq!(pending_reads(&s), 1, "as recusas tiram da lista o que saiu, sem reler");
    assert_eq!(feed_ids(&s.wa), ["d2", "d3"]);
    let feed = s.wa.outbox().feed().borrow().clone().unwrap();
    assert_eq!(feed.drafts[0]["answer_by"], json!("2026-11-24T10:09:30Z"), "o prazo da tool vence antes da expiração");
    assert_eq!(*hooks.new.lock().unwrap(), ["d2", "d3"]);
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert_eq!(*hooks.new.lock().unwrap(), ["d2", "d3"], "o mesmo rascunho não avisa de novo");
    s.call(Method::POST, "/api/whatsapp/outbox/d3/decline", true, None).await;

    let (status, _) = s.call(Method::POST, "/api/whatsapp/outbox/d2/decline", true, None).await;
    assert_eq!(status, 200);
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert!(feed_ids(&s.wa).is_empty(), "respondido sai do evento");
    assert!(s.wa.outbox().feed().borrow().is_some(), "lista vazia lida também é resposta");
    assert!(hooks.failed.lock().unwrap().is_empty());
}

/// Recusa da subida que falha não dá a volta por feita: o rascunho de outra vida aparece no cartão sem avisar o
/// celular, e a volta seguinte tenta de novo.
#[tokio::test]
async fn a_failed_start_decline_is_retried_on_the_next_round() {
    let s = Scene::new();
    s.serve_api(Signing::Signed, 2).await;
    s.drafts.lock().unwrap().push(json!({"id": "d0", "state": "pending", "created_at": "2026-11-24T08:00:00Z",
        "expires_at": "2099-01-01T00:00:00Z", "fail_decline": true}));
    let hooks = Recorded::default();
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert_eq!(*hooks.failed.lock().unwrap(), ["whatsapp_outbox_sweep_failed"]);
    assert_eq!(s.drafts.lock().unwrap()[0]["state"], json!("pending"));
    assert_eq!(feed_ids(&s.wa), ["d0"]);
    assert!(hooks.new.lock().unwrap().is_empty(), "o de outra vida não avisa o celular");

    s.drafts.lock().unwrap()[0]["fail_decline"] = json!(false);
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert_eq!(s.drafts.lock().unwrap()[0]["state"], json!("declined"), "tentou de novo");
    assert!(feed_ids(&s.wa).is_empty());
}

/// Sem a api 2 nada é lido nem avisado; leitura que falha vai uma vez ao diário e a última lista continua.
#[tokio::test]
async fn without_the_send_api_nothing_is_read_and_a_failed_read_keeps_the_list() {
    let s = Scene::new();
    s.serve(Signing::Signed).await;
    let hooks = Recorded::default();
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert!(s.seen.lock().unwrap().is_empty() && feed_ids(&s.wa).is_empty());
    assert!(s.wa.outbox().feed().borrow().is_none(), "sem envio, o evento não sai");

    s.serve_api(Signing::Signed, 2).await;
    s.call(Method::POST, "/api/whatsapp/outbox", true, Some(r#"{"chat":"5564999","text":"oi","origin":"casa::s0"}"#)).await;
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert_eq!(feed_ids(&s.wa), ["d1"]);

    s.serve_api(Signing::Unsigned, 2).await;
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert_eq!(*hooks.failed.lock().unwrap(), ["whatsapp_untrusted"], "uma vez por falha");
    assert_eq!(feed_ids(&s.wa), ["d1"], "a última lista fica");

    s.write_control_api(1, DEAD_PID, 2);
    s.wa.outbox.refresh(&s.wa.client, &hooks).await;
    assert_eq!(s.wa.outbox().feed().borrow().as_deref(), Some(&outbox::Feed::default()), "serviço desligado: o cartão na tela some");
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
