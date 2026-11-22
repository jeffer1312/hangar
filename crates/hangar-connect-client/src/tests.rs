//! Os casos de `packages/core/src/connectAccount.test.ts`, pelo dublê do transporte.
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use crate::crypto::{self, Keys};
use crate::fake::{Fake, body, header, host, json, path};
use crate::{
    AccountSession, Code, ConnectClient, ConnectMachine, DeviceKey, DeviceStatus, Error, NameLookup, Request, Response, SignInResult,
    VaultServer,
};

const SERVICE: &str = "https://connect.hangar.dev.br";
const SERVICE_HOST: &str = "connect.hangar.dev.br";
const PASSWORD: &str = "senha-da-lhais";
const SALT: &str = "c2Fs";

fn key_for(password: &str) -> [u8; 32] {
    ring::digest::digest(&ring::digest::SHA256, password.as_bytes()).as_ref().try_into().unwrap()
}

// Conta barata no lugar das 600 mil voltas; o par real tem prova própria (`real_keys_open_the_vault`).
fn cheap(password: &str, salt: &[u8], iterations: u32) -> Keys {
    Keys { auth_hash: format!("{password}|{}|{iterations}", STANDARD.encode(salt)), enc_key: key_for(password) }
}

fn client(handler: impl Fn(&Request) -> Result<Response, Error> + Send + Sync + 'static) -> ConnectClient<Fake> {
    let mut client = ConnectClient::new(Fake::new(handler));
    client.derive = cheap;
    client
}

fn session() -> AccountSession {
    AccountSession {
        service_url: SERVICE.into(), token: "account-token".into(),
        device: Some(DeviceKey { id: "dev1".into(), secret: "devsecret".into() }),
    }
}

fn vault() -> Value {
    json!([
        {"id": "a", "label": "Casa", "baseUrl": "", "token": "ta"},
        {"id": "b", "label": "PC", "baseUrl": "https://pc.ana.hangar.dev.br", "token": "tb"},
        {"id": "c", "label": "Desligada", "baseUrl": "endereço torto", "token": "tc", "disabled": true},
    ])
}

fn seal(key: &[u8; 32], list: &Value) -> Value {
    serde_json::to_value(crypto::encrypt(key, list.to_string().as_bytes()).unwrap()).unwrap()
}

#[derive(Clone, Default)]
struct Machine {
    password: &'static str,
    down: bool,
    limited: bool,
    /// `None` = o cofre de verdade, cifrado com a senha.
    blob: Option<Value>,
}

/// Serviço com o `lookup` dado e as máquinas que guardam o cofre da convidada.
fn guests(hosts: Vec<(&'static str, Machine)>, lookup: Option<Value>) -> ConnectClient<Fake> {
    let lookup = lookup.unwrap_or_else(|| json!({"kind": "guest", "hosts": hosts.iter().map(|(host, _)| *host).collect::<Vec<_>>()}));
    client(move |request| {
        let (host, path) = (host(request), path(request));
        if host == SERVICE_HOST {
            return if path == "/v1/lookup" { json(200, lookup.clone()) } else { json(401, json!({})) };
        }
        let Some((_, machine)) = hosts.iter().find(|(name, _)| *name == host) else { return Err(Error::network("sem rota")) };
        if machine.down {
            return Err(Error::network("fora do ar"));
        }
        let cookie = format!("cp_sync=\"s-{host}\"");
        match path.as_str() {
            "/api/sync/prelogin" => json(200, json!({"salt": SALT, "iterations": 600_000})),
            "/api/sync/login" if machine.limited => json(429, json!({})),
            "/api/sync/login" if body(request)["auth_hash"] == format!("{}|{SALT}|600000", machine.password) => Ok(Response {
                status: 200, set_cookies: vec![format!("{cookie}; HttpOnly; Max-Age=2592000; Path=/; SameSite=lax")],
                body: b"{\"ok\":true}".to_vec(),
            }),
            "/api/sync/login" => json(401, json!({})),
            "/api/sync/logout" => json(200, json!({"ok": true})),
            "/api/sync/vault" if header(request, "Cookie") != Some(cookie.as_str()) => json(401, json!({})),
            "/api/sync/vault" => {
                let blob = machine.blob.clone().unwrap_or_else(|| seal(&key_for(machine.password), &vault()));
                json(200, json!({"enc_blob": blob, "rev": 1}))
            }
            _ => json(404, json!({})),
        }
    })
}

fn code<T>(result: Result<T, Error>) -> Code {
    result.err().expect("devia falhar").code()
}

#[tokio::test]
async fn lookup_tells_account_guest_and_none_apart_with_the_name_in_lowercase() {
    let answers = Mutex::new(VecDeque::from([
        json!({"kind": "account"}), json!({"kind": "none"}), json!({"kind": "guest", "hosts": ["casa.ana.hangar.dev.br"]}),
    ]));
    let client = client(move |_| json(200, answers.lock().unwrap().pop_front().unwrap()));
    assert_eq!(client.lookup_name(SERVICE, " ACME ").await.unwrap(), NameLookup::Account);
    assert_eq!(client.lookup_name(SERVICE, "ninguem@casa").await.unwrap(), NameLookup::None);
    assert_eq!(client.lookup_name(SERVICE, "Lhais@Casa").await.unwrap(), NameLookup::Guest { hosts: vec!["casa.ana.hangar.dev.br".into()] });
    let calls = client.transport.calls();
    assert_eq!(calls.iter().map(body).collect::<Vec<_>>(), [json!({"name": "acme"}), json!({"name": "ninguem@casa"}), json!({"name": "lhais@casa"})]);
    assert_eq!(header(&calls[0], "Content-Type"), Some("application/json"));
}

#[tokio::test]
async fn lookup_without_the_directory_is_an_account_and_429_has_its_own_code() {
    let old = client(|_| json(404, json!({})));
    assert_eq!(old.lookup_name(SERVICE, "acme").await.unwrap(), NameLookup::Account);
    let busy = client(|_| json(429, json!({})));
    let error = busy.lookup_name(SERVICE, "acme").await.unwrap_err();
    assert_eq!((error.code(), error.status()), (Code::ConnectTooManyAttempts, Some(429)));
}

#[tokio::test]
async fn lookup_refuses_hosts_out_of_shape_and_no_answer_is_network() {
    for bad in [json!({"kind": "guest", "hosts": []}), json!({"kind": "guest", "hosts": ["https://x.dev"]}), json!({"kind": "guest"}),
        json!({"kind": "guest", "hosts": ["Casa.dev"]}), json!({"kind": "outro"}), json!([1])] {
        let client = client(move |_| json(200, bad.clone()));
        assert_eq!(code(client.lookup_name(SERVICE, "ana").await), Code::ConnectResponseInvalid);
    }
    let down = client(|_| Err(Error::network("sem rede")));
    assert_eq!(code(down.lookup_name(SERVICE, "ana").await), Code::NetworkFailed);
}

#[tokio::test]
async fn http_service_outside_loopback_is_refused_before_any_request() {
    let client = client(|_| json(200, json!({"token": "t"})));
    assert_eq!(code(client.sign_in("http://connect.hangar.dev.br", "acme", "senha").await), Code::ConnectServiceInsecure);
    assert!(client.transport.calls().is_empty());
    assert!(client.sign_in("http://127.0.0.1:9", "acme", "senha").await.is_ok());
}

#[tokio::test]
async fn account_signs_in_through_the_account_login() {
    let client = client(|request| if path(request) == "/v1/lookup" { json(200, json!({"kind": "account"})) } else { json(201, json!({"token": "account-token"})) });
    let result = client.sign_in_anywhere(SERVICE, "ACME", "senha").await.unwrap();
    assert_eq!(result, SignInResult::Account(AccountSession { service_url: SERVICE.into(), token: "account-token".into(), device: None }));
    let calls = client.transport.calls();
    assert_eq!(calls.iter().map(path).collect::<Vec<_>>(), ["/v1/lookup", "/v1/sessions"]);
    assert_eq!(body(&calls[1]), json!({"name": "acme", "password": "senha"}));
}

#[tokio::test]
async fn refused_account_password_does_not_match() {
    let client = client(|request| if path(request) == "/v1/lookup" { json(200, json!({"kind": "account"})) } else { json(401, json!({})) });
    let error = client.sign_in_anywhere(SERVICE, "acme", "errada").await.unwrap_err();
    assert_eq!((error.code(), error.status()), (Code::CredentialsMismatch, Some(401)));
}

#[tokio::test]
async fn unknown_name_does_not_match_and_the_password_stays_here() {
    let client = guests(Vec::new(), Some(json!({"kind": "none"})));
    assert_eq!(code(client.sign_in_anywhere(SERVICE, "ninguem@casa", PASSWORD).await), Code::CredentialsMismatch);
    assert_eq!(client.transport.routes(), ["POST connect.hangar.dev.br/v1/lookup"]);
    assert!(!format!("{:?}", client.transport.calls().iter().map(body).collect::<Vec<_>>()).contains(PASSWORD));
}

#[tokio::test]
async fn guest_enters_by_the_machine_that_keeps_the_vault() {
    let client = guests(vec![("casa.ana.hangar.dev.br", Machine { password: PASSWORD, ..Machine::default() })], None);
    let result = client.sign_in_anywhere(SERVICE, "Lhais@Casa", PASSWORD).await.unwrap();
    let SignInResult::Guest { host: vault_host, servers } = result else { panic!("devia ser convidada") };
    assert_eq!(vault_host, "casa.ana.hangar.dev.br");
    // Endereço vazio no cofre foi gravado de dentro da própria máquina; a desligada fica fora.
    assert_eq!(servers, [
        VaultServer { id: "a".into(), label: "Casa".into(), base_url: "https://casa.ana.hangar.dev.br".into(), token: "ta".into() },
        VaultServer { id: "b".into(), label: "PC".into(), base_url: "https://pc.ana.hangar.dev.br".into(), token: "tb".into() },
    ]);
    assert_eq!(client.transport.routes(), [
        "POST connect.hangar.dev.br/v1/lookup", "GET casa.ana.hangar.dev.br/api/sync/prelogin", "POST casa.ana.hangar.dev.br/api/sync/login",
        "GET casa.ana.hangar.dev.br/api/sync/vault", "POST casa.ana.hangar.dev.br/api/sync/logout",
    ]);
    let calls = client.transport.calls();
    assert!(calls[1].url.ends_with("/api/sync/prelogin?user=lhais%40casa"), "{}", calls[1].url);
    assert_eq!(body(&calls[2]), json!({"user": "lhais@casa", "auth_hash": format!("{PASSWORD}|{SALT}|600000")}));
    // O cookie do login vale para a leitura e para a saída, e só elas.
    assert_eq!(header(&calls[4], "Cookie"), Some("cp_sync=\"s-casa.ana.hangar.dev.br\""));
    assert_eq!(header(&calls[2], "Cookie"), None);
    let to_service: Vec<Value> = calls.iter().filter(|call| host(call) == SERVICE_HOST).map(body).collect();
    assert_eq!(to_service, [json!({"name": "lhais@casa"})]);
}

#[tokio::test]
async fn one_machine_at_a_time_stopping_at_the_first_that_accepts() {
    let client = guests(vec![
        ("casa.ana.hangar.dev.br", Machine { down: true, ..Machine::default() }),
        ("pc.ana.hangar.dev.br", Machine { password: PASSWORD, ..Machine::default() }),
        ("velho.ana.hangar.dev.br", Machine { password: PASSWORD, ..Machine::default() }),
    ], None);
    let result = client.sign_in_anywhere(SERVICE, "lhais@casa", PASSWORD).await.unwrap();
    assert!(matches!(result, SignInResult::Guest { ref host, .. } if host == "pc.ana.hangar.dev.br"));
    assert!(client.transport.calls().iter().all(|call| host(call) != "velho.ana.hangar.dev.br"));
}

#[tokio::test]
async fn refused_password_on_one_machine_moves_to_the_next() {
    let client = guests(vec![
        ("casa.ana.hangar.dev.br", Machine { password: "outra", ..Machine::default() }),
        ("pc.ana.hangar.dev.br", Machine { password: PASSWORD, ..Machine::default() }),
    ], None);
    assert!(matches!(client.sign_in_anywhere(SERVICE, "lhais@casa", PASSWORD).await.unwrap(),
        SignInResult::Guest { ref host, .. } if host == "pc.ana.hangar.dev.br"));
}

#[tokio::test]
async fn every_machine_refusing_does_not_match_but_one_down_is_network() {
    let refused = guests(vec![
        ("casa.ana.hangar.dev.br", Machine { password: "outra", ..Machine::default() }),
        ("pc.ana.hangar.dev.br", Machine { password: "outra", ..Machine::default() }),
    ], None);
    assert_eq!(code(refused.sign_in_anywhere(SERVICE, "lhais@casa", PASSWORD).await), Code::CredentialsMismatch);
    let down = guests(vec![
        ("casa.ana.hangar.dev.br", Machine { password: "outra", ..Machine::default() }),
        ("pc.ana.hangar.dev.br", Machine { down: true, ..Machine::default() }),
    ], None);
    assert_eq!(code(down.sign_in_anywhere(SERVICE, "lhais@casa", PASSWORD).await), Code::NetworkFailed);
}

#[tokio::test]
async fn attempt_limit_has_its_own_code() {
    let client = guests(vec![("casa.ana.hangar.dev.br", Machine { limited: true, ..Machine::default() })], None);
    let error = client.sign_in_anywhere(SERVICE, "lhais@casa", PASSWORD).await.unwrap_err();
    assert_eq!((error.code(), error.status()), (Code::SyncTooManyAttempts, Some(429)));
}

#[tokio::test]
async fn unreadable_vault_is_not_an_empty_list_and_the_session_ends() {
    let wrong = seal(&key_for("outra"), &vault());
    let client = guests(vec![("casa.ana.hangar.dev.br", Machine { password: PASSWORD, blob: Some(wrong), ..Machine::default() })], None);
    assert_eq!(code(client.sign_in_anywhere(SERVICE, "lhais@casa", PASSWORD).await), Code::SyncVaultUnreadable);
    assert!(client.transport.routes().contains(&"POST casa.ana.hangar.dev.br/api/sync/logout".to_owned()));
}

#[tokio::test]
async fn item_out_of_shape_makes_the_whole_vault_unreadable() {
    let broken = json!([{"id": "a b", "label": "Casa", "baseUrl": "https://casa.dev", "token": "t"}]);
    let client = guests(vec![("casa.ana.hangar.dev.br", Machine {
        password: PASSWORD, blob: Some(seal(&key_for(PASSWORD), &broken)), ..Machine::default()
    })], None);
    assert_eq!(code(client.sign_in_anywhere(SERVICE, "lhais@casa", PASSWORD).await), Code::SyncVaultUnreadable);
}

#[tokio::test]
async fn empty_vault_enters_without_machines() {
    let client = guests(vec![("casa.ana.hangar.dev.br", Machine { password: PASSWORD, blob: Some(Value::Null), ..Machine::default() })], None);
    assert_eq!(client.sign_in_anywhere(SERVICE, "lhais@casa", PASSWORD).await.unwrap(),
        SignInResult::Guest { host: "casa.ana.hangar.dev.br".into(), servers: Vec::new() });
}

#[tokio::test]
async fn sync_login_refuses_http_and_a_cheap_prelogin() {
    let client = client(|_| json(200, json!({"salt": SALT, "iterations": 1000})));
    assert_eq!(code(client.sync_sign_in("http://casa.dev", "a@b", PASSWORD).await), Code::SyncInsecure);
    assert!(client.transport.calls().is_empty());
    assert_eq!(code(client.sync_sign_in("casa.dev", "a@b", PASSWORD).await), Code::SyncUnavailable);
    assert_eq!(client.transport.routes(), ["GET casa.dev/api/sync/prelogin"], "o login não pode sair com custo baixo");
}

#[tokio::test]
async fn real_keys_open_the_vault_sealed_by_the_machine() {
    let keys = crypto::derive_keys(PASSWORD, b"sal", crypto::ITERATIONS);
    let (auth, blob) = (keys.auth_hash.clone(), seal(&keys.enc_key, &vault()));
    let client = ConnectClient::new(Fake::new(move |request| match path(request).as_str() {
        "/api/sync/prelogin" => json(200, json!({"salt": SALT, "iterations": 600_000})),
        "/api/sync/login" if body(request)["auth_hash"] == auth.as_str() => json(200, json!({"ok": true})),
        "/api/sync/login" => json(401, json!({})),
        "/api/sync/vault" => json(200, json!({"enc_blob": blob.clone()})),
        _ => json(200, json!({})),
    }));
    let servers = client.sync_sign_in("https://casa.ana.hangar.dev.br", "lhais@casa", PASSWORD).await.unwrap();
    assert_eq!(servers.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
}

/// Serviço que dá o bilhete e máquina que responde à espera pela fila `status`.
fn machine_flow(status: Vec<Result<Response, Error>>, ticket_url: &'static str) -> ConnectClient<Fake> {
    let status = Mutex::new(VecDeque::from(status));
    client(move |request| match (host(request).as_str(), path(request).as_str()) {
        (SERVICE_HOST, "/v1/machines/m1/devices") => json(200, json!({"url": ticket_url, "ticket": "tk", "signature": "sg"})),
        (_, "/api/connect/redeem") => json(200, json!({"device_id": "d1"})),
        (_, "/api/connect/redeem/status") => {
            let mut status = status.lock().unwrap();
            if status.len() > 1 { status.pop_front().unwrap() } else { status.front().cloned().unwrap() }
        }
        _ => json(404, json!({})),
    })
}

fn machine() -> ConnectMachine {
    ConnectMachine { machine_id: "m1".into(), name: "Casa".into(), host: "casa.ana.hangar.dev.br".into(), online: true }
}

#[tokio::test]
async fn pre_approved_ticket_delivers_the_token_without_waiting_or_notice() {
    let client = machine_flow(vec![json(200, json!({"status": "approved", "token": "device-token"}))], "https://casa.ana.hangar.dev.br/");
    let pending = AtomicUsize::new(0);
    let entry = client.add_machine(&session(), &machine(), "Hangar PC", Some("inst-1"), || { pending.fetch_add(1, Ordering::SeqCst); })
        .await.unwrap();
    assert_eq!((entry.base_url.as_str(), entry.token.as_str(), entry.device_id.as_str()), ("https://casa.ana.hangar.dev.br", "device-token", "d1"));
    assert_eq!((entry.id.as_str(), entry.label.as_str()), ("m1", "Casa"));
    assert_eq!(pending.load(Ordering::SeqCst), 0);
    assert_eq!(client.transport.slept(), Duration::ZERO);
    let calls = client.transport.calls();
    assert_eq!(client.transport.routes(), [
        "POST connect.hangar.dev.br/v1/machines/m1/devices", "POST casa.ana.hangar.dev.br/api/connect/redeem",
        "POST casa.ana.hangar.dev.br/api/connect/redeem/status",
    ]);
    // Ao serviço vai só o hash do segredo, e a prova do aparelho; à máquina, o segredo e nenhum token da conta.
    let secret = body(&calls[1])["secret"].as_str().unwrap().to_owned();
    let hash: String = ring::digest::digest(&ring::digest::SHA256, secret.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(secret.len(), 64);
    assert_eq!(body(&calls[0]), json!({"device_name": "Hangar PC", "verifier": hash}));
    assert_eq!(header(&calls[0], "X-Hangar-Device"), Some("dev1"));
    assert_eq!(body(&calls[1]), json!({"ticket": "tk", "signature": "sg", "secret": secret, "device_name": "Hangar PC", "install_id": "inst-1"}));
    assert!(calls[1..].iter().all(|call| header(call, "Authorization").is_none()));
    assert_eq!(body(&calls[2]), json!({"device_id": "d1", "secret": secret}));
}

#[tokio::test]
async fn old_machine_waits_every_3_s_and_says_once_that_it_asked() {
    let client = machine_flow(vec![
        json(200, json!({"status": "pending"})), json(503, json!({})), Err(Error::network("caiu")), json(429, json!({})),
        json(200, json!({"status": "approved", "token": "t"})),
    ], "https://casa.ana.hangar.dev.br");
    let pending = AtomicUsize::new(0);
    let entry = client.add_machine(&session(), &machine(), "PC", None, || { pending.fetch_add(1, Ordering::SeqCst); }).await.unwrap();
    assert_eq!(entry.token, "t");
    assert_eq!(pending.load(Ordering::SeqCst), 1);
    assert_eq!(client.transport.slept(), Duration::from_secs(12));
    assert!(body(&client.transport.calls()[1]).get("install_id").is_none());
}

#[tokio::test]
async fn waiting_ends_at_10_minutes() {
    let client = machine_flow(vec![json(200, json!({"status": "pending"}))], "https://casa.ana.hangar.dev.br");
    assert_eq!(code(client.add_machine(&session(), &machine(), "PC", None, || {}).await), Code::ConnectApprovalExpired);
    assert_eq!(client.transport.slept(), Duration::from_secs(600));
    let polls = client.transport.routes().iter().filter(|route| route.ends_with("/redeem/status")).count();
    assert_eq!(polls, 201);
}

#[tokio::test]
async fn denial_lost_token_and_a_refusal_end_the_wait() {
    let denied = machine_flow(vec![json(200, json!({"status": "denied"}))], "https://casa.ana.hangar.dev.br");
    assert_eq!(code(denied.add_machine(&session(), &machine(), "PC", None, || {}).await), Code::ConnectDeviceDenied);
    let lost = machine_flow(vec![json(200, json!({"status": "approved"}))], "https://casa.ana.hangar.dev.br");
    assert_eq!(code(lost.add_machine(&session(), &machine(), "PC", None, || {}).await), Code::ConnectTokenLost);
    let refused = machine_flow(vec![json(404, json!({}))], "https://casa.ana.hangar.dev.br");
    let error = refused.add_machine(&session(), &machine(), "PC", None, || {}).await.unwrap_err();
    assert_eq!((error.code(), error.status()), (Code::ConnectRequestFailed, Some(404)));
}

#[tokio::test]
async fn ticket_for_another_host_or_http_is_refused_before_the_secret_leaves() {
    for url in ["https://outra.ana.hangar.dev.br", "http://casa.ana.hangar.dev.br"] {
        let client = machine_flow(vec![json(200, json!({"status": "approved", "token": "t"}))], url);
        assert_eq!(code(client.add_machine(&session(), &machine(), "PC", None, || {}).await), Code::ConnectResponseInvalid);
        assert_eq!(client.transport.calls().len(), 1, "{url}");
    }
}

#[tokio::test]
async fn device_is_registered_with_the_install_id_and_checked_with_its_proof() {
    let client = client(|request| match (path(request).as_str(), crate::fake::route(request).starts_with("GET")) {
        ("/v1/devices", false) => json(201, json!({"device_id": "dev2", "device_secret": "s2"})),
        ("/v1/devices/dev2", true) => json(200, json!({"status": "approved"})),
        _ => json(404, json!({})),
    });
    let account = AccountSession { device: None, ..session() };
    let key = client.register_device(&account, "Hangar PC", Some("inst-1")).await.unwrap();
    assert_eq!(key, DeviceKey { id: "dev2".into(), secret: "s2".into() });
    assert_eq!(client.device_status(&account, &key).await.unwrap(), DeviceStatus::Approved);
    let calls = client.transport.calls();
    assert_eq!(body(&calls[0]), json!({"name": "Hangar PC", "install_id": "inst-1"}));
    assert_eq!(header(&calls[0], "X-Hangar-Device"), None);
    assert_eq!(header(&calls[1], "Authorization"), Some("Bearer account-token"));
    assert_eq!((header(&calls[1], "X-Hangar-Device"), header(&calls[1], "X-Hangar-Device-Secret")), (Some("dev2"), Some("s2")));
    // Pedido vencido: o 404 chega a quem chama, que registra de novo.
    let gone = client.device_status(&account, &DeviceKey { id: "velho".into(), secret: "x".into() }).await.unwrap_err();
    assert_eq!(gone.status(), Some(404));
}

#[tokio::test]
async fn known_device_reason_becomes_its_code_and_others_the_generic_one() {
    let pending = client(|_| json(403, json!({"reason": "device_pending"})));
    let error = pending.list_machines(&session()).await.unwrap_err();
    assert_eq!((error.code(), error.status()), (Code::ConnectDevicePending, Some(403)));
    let echo = client(|_| json(403, json!({"reason": "segredo ecoado"})));
    assert_eq!(code(echo.list_account_devices(&session()).await), Code::ConnectRequestFailed);
}

#[tokio::test]
async fn machines_are_listed_with_the_account_token_only_and_checked() {
    let rows = json!([{"machine_id": "m1", "name": "Casa", "host": "casa.ana.hangar.dev.br", "online": true}]);
    let client = client(move |_| json(200, rows.clone()));
    assert_eq!(client.list_machines(&session()).await.unwrap(), [machine()]);
    let call = &client.transport.calls()[0];
    assert_eq!((header(call, "Authorization"), header(call, "X-Hangar-Device")), (Some("Bearer account-token"), None));
    let bad = self::client(|_| json(200, json!([{"machine_id": "m1", "name": "", "host": "h", "online": true}])));
    assert_eq!(code(bad.list_machines(&session()).await), Code::ConnectResponseInvalid);
}

#[tokio::test]
async fn account_devices_are_parsed_and_answered() {
    let client = client(|request| match crate::fake::route(request).as_str() {
        "GET connect.hangar.dev.br/v1/devices" => json(200, json!([
            {"device_id": "d1", "name": "iPhone", "status": "pending", "created_at": 1763812345.25, "approved_at": null, "revoked_at": null,
                "install_code": "K7Q2"},
            {"device_id": "d2", "name": "Velho", "status": "approved", "created_at": 1, "approved_at": 2.5, "revoked_at": null},
        ])),
        "POST connect.hangar.dev.br/v1/devices/d%201/approve" | "DELETE connect.hangar.dev.br/v1/devices/d%201" => Ok(Response { status: 204, ..Response::default() }),
        _ => json(404, json!({})),
    });
    let devices = client.list_account_devices(&session()).await.unwrap();
    assert_eq!((devices[0].status, devices[0].created_at, devices[0].approved_at), (DeviceStatus::Pending, 1763812345.25, None));
    assert_eq!((devices[0].install_code.as_deref(), devices[1].install_code.as_deref()), (Some("K7Q2"), None));
    assert_eq!((devices[1].created_at, devices[1].approved_at), (1.0, Some(2.5)));
    client.approve_account_device(&session(), "d 1").await.unwrap();
    client.revoke_account_device(&session(), "d 1").await.unwrap();
}

#[tokio::test]
async fn machine_registration_keeps_the_service_answer() {
    let client = client(|request| {
        assert_eq!(body(request), json!({"name": "notebook"}));
        json(200, json!({
            "machine_id": "notebook", "machine_key": "k", "host": "notebook.ana.hangar.dev.br", "guest_host": "g", "preview_host": "p",
            "frps_addr": "f:7000", "frps_token": "ft", "service_public_key": "pk", "extra": 1,
        }))
    });
    let machine = client.register_machine(&AccountSession { device: None, ..session() }, "notebook").await.unwrap();
    assert_eq!((machine.machine_id.as_str(), machine.machine_key.as_str()), ("notebook", "k"));
    let missing = self::client(|_| json(200, json!({"machine_id": "x"})));
    assert_eq!(code(missing.register_machine(&session(), "x").await), Code::ConnectResponseInvalid);
}

#[test]
fn guest_names_carry_a_dot_or_an_at() {
    assert_eq!(crate::normalize_name("  Lhais87.Oliveira "), "lhais87.oliveira");
    assert!(crate::valid_guest_user("lhais87.oliveira") && crate::valid_guest_user("ana@casa"));
    assert!(!crate::valid_guest_user("acme") && !crate::valid_guest_user("a .b"));
}
