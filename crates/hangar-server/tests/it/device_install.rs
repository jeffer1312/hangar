//! `/api/device-install/*`: login Apple com 2FA e o trabalho que instala o SideStore, com aparelho,
//! Apple e download falsos. Nada aqui sai para a rede.
use std::sync::Arc;
use std::time::Duration;

use crate::fake::*;
use hangar_server::device_install::apple::{ANISETTE, CertInfo, FakeSigner};
use hangar_server::device_install::job::{DeviceInstall, Pinned, SOURCE_URL};
use hangar_server::device_install::{DeviceError, DeviceInfo, FakeLink};
use serde_json::{Value, json};

const UDID: &str = "00008101-00113CEC3CBA601E";
const PASSWORD: &str = "senha-sintetica-9f3k";
const TEAM: &str = "TEAMID1234";
const BUNDLE: &str = "com.SideStore.SideStore.TEAMID1234";
const IPA: &[u8] = b"ipa sintetico";

fn ipad() -> DeviceInfo {
    DeviceInfo { udid: UDID.into(), name: "iPad".into(), ios_version: "26.5".into(), trusted: true, error: None }
}

fn link() -> FakeLink {
    FakeLink { devices: vec![ipad()], ..Default::default() }
}

fn signer() -> FakeSigner {
    FakeSigner { code: "123456".into(), team_id: TEAM.into(), ..Default::default() }
}

fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// IPA servido por um servidor local; `sha` errado simula o arquivo trocado no caminho.
async fn ipa_server(sha: Option<&str>) -> Pinned {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/SideStore.ipa", axum::routing::get(|| async { IPA }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    Pinned {
        url: leak(format!("http://{addr}/SideStore.ipa")),
        sha256: leak(sha.map_or_else(|| sha256(IPA), str::to_owned)),
        size: IPA.len() as u64,
    }
}

struct Server {
    url: String,
    link: Arc<FakeLink>,
    signer: Arc<FakeSigner>,
    dir: tempfile::TempDir,
    python: Arc<Fake>,
}

async fn serve_with(link: FakeLink, signer: FakeSigner, pinned: Pinned, config: Value, code_wait: Duration) -> Server {
    let (python, _) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let (link, signer) = (Arc::new(link), Arc::new(signer));
    let mut state = hangar_server::routes::AppState::new(config("127.0.0.1"));
    state.device_install = Arc::new(
        DeviceInstall::new(link.clone(), signer.clone(), dir.path())
            .with_pinned(pinned)
            .with_config(Arc::new(move || config.clone()))
            .with_code_wait(code_wait),
    );
    let addr = spawn_state_for(&python, state).await;
    Server { url: format!("http://{addr}/api/device-install"), link, signer, dir, python }
}

async fn serve(link: FakeLink, signer: FakeSigner) -> Server {
    serve_with(link, signer, ipa_server(None).await, Value::Null, Duration::from_secs(30)).await
}

impl Server {
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let r = client().post(format!("{}/{path}", self.url)).bearer_auth(OWNER).json(&body).send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        assert!(!text.contains(PASSWORD), "a senha voltou numa resposta: {text}");
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn start(&self) -> String {
        let (status, body) = self.post("start", json!({"udid": UDID, "apple_id": "Pessoa@Example.com", "password": PASSWORD})).await;
        assert_eq!(status, 200, "{body}");
        body["job"].as_str().unwrap().to_string()
    }

    async fn progress(&self, job: &str) -> Value {
        let r = client().get(format!("{}/progress?job={job}", self.url)).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let text = r.text().await.unwrap();
        assert!(!text.contains(PASSWORD), "a senha apareceu no progresso: {text}");
        serde_json::from_str(&text).unwrap()
    }

    /// Espera o trabalho chegar a `stage` (com o erro dado, quando pedido).
    async fn wait(&self, job: &str, stage: &str, error: Option<&str>) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let p = self.progress(job).await;
            if p["stage"] == stage && error.is_none_or(|e| p["error"]["code"] == e) {
                return p;
            }
            assert!(!(p["stage"] == "failed" && stage != "failed"), "falhou antes de {stage}: {p}");
            assert!(std::time::Instant::now() < deadline, "não chegou a {stage} {error:?}: {p}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn code(&self, job: &str, body: Value) {
        let mut body = body;
        body["job"] = json!(job);
        let (status, reply) = self.post("code", body).await;
        assert_eq!(status, 200, "{reply}");
    }
}

#[tokio::test]
async fn happy_path_reaches_done_after_the_device_confirms_the_install() {
    let l = link();
    l.installed.lock().unwrap().push(BUNDLE.into());
    let s = serve(l, signer()).await;
    let job = s.start().await;

    let p = s.wait(&job, "awaiting_code", None).await;
    assert_eq!(p, json!({"stage": "awaiting_code", "error": null, "source_url": SOURCE_URL}));
    assert_eq!(SOURCE_URL, "https://github.com/jeffer1312/hangar/releases/download/mobile-latest/sidestore-source.json");

    s.code(&job, json!({"code": "000000"})).await;
    s.wait(&job, "awaiting_code", Some("device_install_wrong_code")).await;
    s.code(&job, json!({"resend": true})).await;
    s.wait(&job, "awaiting_code", None).await;
    s.code(&job, json!({"code": "123456"})).await;
    let done = s.wait(&job, "done", None).await;
    assert_eq!(done["error"], Value::Null);

    assert_eq!(*s.signer.logins.lock().unwrap(), [ANISETTE[0]], "a senha foi mandada uma vez só");
    assert_eq!(*s.signer.resends.lock().unwrap(), 1);
    assert_eq!(*s.signer.signed.lock().unwrap(), [(UDID.to_string(), format!("pairing-{UDID}").into_bytes())]);
    assert_eq!(s.link.installs.lock().unwrap().len(), 1);
    assert_eq!(*s.link.pairings.lock().unwrap(), [(UDID.to_string(), BUNDLE.to_string())]);
    assert_eq!(*s.link.prefs.lock().unwrap(), [(UDID.to_string(), BUNDLE.to_string())]);
    assert_no_password_on_disk(s.dir.path());
    assert_eq!(s.python.hits_to("/api/device-install/start"), 0, "nada vai ao Python");
}

#[tokio::test]
async fn install_that_the_device_does_not_report_is_not_done() {
    let s = serve(link(), signer()).await;
    let job = s.start().await;
    s.wait(&job, "awaiting_code", None).await;
    s.code(&job, json!({"code": "123456"})).await;
    let p = s.wait(&job, "failed", Some("device_install_not_installed")).await;
    assert_eq!(p["error"]["detail"], BUNDLE);
    assert_eq!(s.link.installs.lock().unwrap().len(), 1, "instalou, mas o aparelho não confirmou");
    assert_eq!(s.link.pairings.lock().unwrap().len(), 1, "pareamento gravado antes de abrir o SideStore");
}

#[tokio::test]
async fn unplugged_device_fails_as_disconnected() {
    let s = serve(FakeLink { devices: vec![ipad()], error: Some(DeviceError::Disconnected), ..Default::default() }, signer()).await;
    let job = s.start().await;
    s.wait(&job, "failed", Some("device_install_disconnected")).await;
    assert!(s.signer.logins.lock().unwrap().is_empty(), "a senha não sai com o aparelho fora");
}

#[tokio::test]
async fn three_apps_stop_before_signing() {
    let s = serve(FakeLink { devices: vec![ipad()], dev_apps: 3, ..Default::default() }, signer()).await;
    let job = s.start().await;
    s.wait(&job, "awaiting_code", None).await;
    s.code(&job, json!({"code": "123456"})).await;
    s.wait(&job, "failed", Some("device_install_app_limit")).await;
    assert!(s.signer.signed.lock().unwrap().is_empty());
    assert!(s.link.installs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn anisette_down_tries_the_backups_then_gives_up() {
    let mine = "https://anisette.example";
    let config = json!({"device_install_anisette": mine});
    let mut down = signer();
    down.down = vec![mine.into(), ANISETTE[0].into()];
    let s = serve_with(link(), down, ipa_server(None).await, config.clone(), Duration::from_secs(30)).await;
    let job = s.start().await;
    s.wait(&job, "awaiting_code", None).await;
    assert_eq!(*s.signer.tried.lock().unwrap(), [mine, ANISETTE[0], ANISETTE[1]]);
    assert_eq!(*s.signer.logins.lock().unwrap(), [ANISETTE[1]]);

    let mut all_down = signer();
    all_down.down = std::iter::once(mine).chain(ANISETTE).map(str::to_owned).collect();
    let s = serve_with(link(), all_down, ipa_server(None).await, config, Duration::from_secs(30)).await;
    let job = s.start().await;
    s.wait(&job, "failed", Some("device_install_anisette_unreachable")).await;
    assert!(s.signer.logins.lock().unwrap().is_empty());
}

#[tokio::test]
async fn wrong_sha256_is_refused_before_signing() {
    let s = serve_with(link(), signer(), ipa_server(Some(&"0".repeat(64))).await, Value::Null, Duration::from_secs(30)).await;
    let job = s.start().await;
    s.wait(&job, "awaiting_code", None).await;
    s.code(&job, json!({"code": "123456"})).await;
    s.wait(&job, "failed", Some("device_install_download_mismatch")).await;
    assert!(s.signer.signed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn no_code_in_time_fails_with_its_own_code() {
    let s = serve_with(link(), signer(), ipa_server(None).await, Value::Null, Duration::from_millis(100)).await;
    let job = s.start().await;
    s.wait(&job, "failed", Some("device_install_code_timeout")).await;
    let (status, body) = s.post("code", json!({"job": job, "code": "123456"})).await;
    assert_eq!((status, body["error"]["code"].as_str()), (409, Some("device_install_not_waiting")));
}

#[tokio::test]
async fn second_start_on_the_same_device_is_busy() {
    let s = serve(link(), signer()).await;
    s.start().await;
    let (status, body) = s.post("start", json!({"udid": UDID, "apple_id": "a@b.c", "password": PASSWORD})).await;
    assert_eq!((status, body["error"]["code"].as_str()), (409, Some("device_install_busy")));
}

#[tokio::test]
async fn devices_and_trust() {
    let s = serve(FakeLink { devices: vec![DeviceInfo { trusted: false, ..ipad() }], ..Default::default() }, signer()).await;
    let r = client().get(format!("{}/devices", s.url)).bearer_auth(OWNER).send().await.unwrap();
    let body: Value = r.json().await.unwrap();
    assert_eq!(body, json!({"service": "ok", "devices": [{"udid": UDID, "name": "iPad", "ios_version": "26.5", "trusted": false}]}));
    assert_eq!(s.post("trust", json!({"udid": UDID})).await.0, 200);
    assert_eq!(*s.link.trust_requests.lock().unwrap(), [UDID]);

    let s = serve(FakeLink { error: Some(DeviceError::ServiceMissing), ..Default::default() }, signer()).await;
    let body: Value = client().get(format!("{}/devices", s.url)).bearer_auth(OWNER).send().await.unwrap().json().await.unwrap();
    assert_eq!(body, json!({"service": "missing", "devices": []}));
}

#[tokio::test]
async fn only_the_owner_token_and_never_a_forwarded_request() {
    let s = serve(link(), signer()).await;
    let devices = format!("{}/devices", s.url);
    assert_eq!(client().get(&devices).send().await.unwrap().status(), 401, "loopback sem token");
    let r = client().get(&devices).bearer_auth("outro").send().await.unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "unauthorized");
    let (status, body) = s.post("trust", json!({"sem_udid": true})).await;
    assert_eq!((status, body["error"]["code"].clone()), (422, json!("device_install_bad_request")));
    for (name, value) in [("x-forwarded-for", "192.0.2.1"), ("x-hangar-guest", "assinado"), ("origin", "http://evil.example")] {
        let r = client().get(&devices).bearer_auth(OWNER).header(name, value).send().await.unwrap();
        assert_eq!(r.status(), 403, "{name}");
        let body: Value = r.json().await.unwrap();
        assert_eq!(body["error"]["code"], "device_install_forbidden");
    }
    let host = devices.trim_start_matches("http://").split('/').next().unwrap().to_string();
    let r = client().get(&devices).bearer_auth(OWNER).header("origin", format!("http://{host}")).send().await.unwrap();
    assert_eq!(r.status(), 200, "a página servida por este mesmo endereço passa");

    let r = client().post(format!("{}/start", s.url)).header("x-forwarded-for", "192.0.2.1").bearer_auth(OWNER)
        .json(&json!({"udid": UDID, "apple_id": "a@b.c", "password": PASSWORD})).send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert!(!r.text().await.unwrap().contains(PASSWORD));
    assert!(s.signer.logins.lock().unwrap().is_empty());
    assert_eq!(s.python.hits_to("/api/device-install/devices"), 0);
}

/// Onde o fake extrai o IPA do trabalho (o isideload extrai no temp do sistema).
fn extracted(s: &Server) -> std::path::PathBuf {
    s.dir.path().join("devices/SideStore.ipa_extracted")
}

async fn sign_and_wait(s: &Server, stage: &str, error: &str) -> Value {
    let job = s.start().await;
    s.wait(&job, "awaiting_code", None).await;
    s.code(&job, json!({"code": "123456"})).await;
    s.wait(&job, stage, Some(error)).await
}

#[tokio::test]
async fn a_panic_in_signing_fails_the_job_and_frees_the_device() {
    let s = serve(link(), FakeSigner { sign_panics: true, ..signer() }).await;
    let p = sign_and_wait(&s, "failed", "device_error").await;
    assert_eq!(p["error"]["detail"], "panic: fake isideload panic");
    assert!(!extracted(&s).exists(), "a pasta com a chave do certificado ficou no disco");
    let (status, body) = s.post("start", json!({"udid": UDID, "apple_id": "a@b.c", "password": PASSWORD})).await;
    assert_eq!(status, 200, "o aparelho continuou ocupado: {body}");
}

#[tokio::test]
async fn failed_or_finished_signing_leaves_no_extracted_app() {
    let s = serve(link(), FakeSigner { sign_error: Some(hangar_server::device_install::job::Failure::new("device_install_sign_failed")), ..signer() }).await;
    sign_and_wait(&s, "failed", "device_install_sign_failed").await;
    assert!(!extracted(&s).exists(), "falha no meio da assinatura deixou a extração");

    let s = serve(link(), signer()).await;
    sign_and_wait(&s, "failed", "device_install_not_installed").await;
    assert_eq!(s.link.installs.lock().unwrap().len(), 1);
    assert!(!extracted(&s).exists(), "instalado, a extração tinha de sumir");
}

#[tokio::test]
async fn two_devices_never_sign_at_the_same_time() {
    let other = DeviceInfo { udid: "00008101-0000000000000002".into(), ..ipad() };
    let l = FakeLink { devices: vec![ipad(), other.clone()], ..Default::default() };
    l.installed.lock().unwrap().push(BUNDLE.into());
    let s = serve(l, FakeSigner { sign_delay: Duration::from_millis(200), ..signer() }).await;
    let mut jobs = Vec::new();
    for udid in [UDID, other.udid.as_str()] {
        let (status, body) = s.post("start", json!({"udid": udid, "apple_id": "a@b.c", "password": PASSWORD})).await;
        assert_eq!(status, 200, "{body}");
        jobs.push(body["job"].as_str().unwrap().to_string());
    }
    for job in &jobs {
        s.wait(job, "awaiting_code", None).await;
        s.code(job, json!({"code": "123456"})).await;
    }
    for job in &jobs {
        s.wait(job, "done", None).await;
    }
    assert_eq!(s.signer.signing.1.load(std::sync::atomic::Ordering::SeqCst), 1, "duas assinaturas juntas dividiriam a pasta do temp");
}

fn full_account() -> FakeSigner {
    let cert = |serial: &str, machine: &str| CertInfo { name: "iPhone Developer".into(), machine_name: machine.into(), serial: serial.into() };
    FakeSigner { cert_limit: vec![cert("AAA111", "SideStore"), cert("BBB222", "iloader")], ..signer() }
}

#[tokio::test]
async fn full_certificates_pause_and_revoking_resumes_to_done() {
    let l = link();
    l.installed.lock().unwrap().push(BUNDLE.into());
    let s = serve(l, full_account()).await;
    let job = s.start().await;
    s.wait(&job, "awaiting_code", None).await;
    let (status, body) = s.post("certs", json!({"job": job, "revoke": ["AAA111"]})).await;
    assert_eq!((status, body["error"]["code"].as_str()), (409, Some("device_install_not_waiting")), "fora da etapa");
    s.code(&job, json!({"code": "123456"})).await;
    let p = s.wait(&job, "awaiting_cert_choice", None).await;
    assert_eq!(p["certs"], json!([
        {"name": "iPhone Developer", "machine_name": "SideStore", "serial": "AAA111"},
        {"name": "iPhone Developer", "machine_name": "iloader", "serial": "BBB222"},
    ]));
    let (status, _) = s.post("certs", json!({"job": job, "revoke": ["ZZZ999"]})).await;
    assert_eq!(status, 422, "serial que a tela não mostrou");
    let (status, _) = s.post("certs", json!({"job": job})).await;
    assert_eq!(status, 422, "nem revogar nem cancelar");
    let (status, body) = s.post("certs", json!({"job": job, "revoke": ["AAA111"]})).await;
    assert_eq!(status, 200, "{body}");
    let done = s.wait(&job, "done", None).await;
    assert!(done.get("certs").is_none(), "{done}");
    assert_eq!(*s.signer.revoked.lock().unwrap(), ["AAA111"]);
    assert_eq!(s.link.installs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelling_the_certificate_choice_fails_with_cert_limit() {
    let s = serve(link(), full_account()).await;
    let job = s.start().await;
    s.wait(&job, "awaiting_code", None).await;
    s.code(&job, json!({"code": "123456"})).await;
    s.wait(&job, "awaiting_cert_choice", None).await;
    let (status, body) = s.post("certs", json!({"job": job, "cancel": true})).await;
    assert_eq!(status, 200, "{body}");
    s.wait(&job, "failed", Some("device_install_cert_limit")).await;
    assert!(s.signer.revoked.lock().unwrap().is_empty());
    assert!(s.link.installs.lock().unwrap().is_empty());
    let (status, _) = s.post("certs", json!({"job": job, "cancel": true})).await;
    assert_eq!(status, 409);
    let (status, _) = s.post("certs", json!({"job": "nenhum", "cancel": true})).await;
    assert_eq!(status, 404);
}

fn assert_no_password_on_disk(dir: &std::path::Path) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            assert_no_password_on_disk(&path);
        } else {
            let bytes = std::fs::read(&path).unwrap();
            assert!(!bytes.windows(PASSWORD.len()).any(|w| w == PASSWORD.as_bytes()), "senha em {}", path.display());
        }
    }
}
