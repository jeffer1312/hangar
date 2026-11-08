//! Binários, configuração, ligação e filhos do Connect. Serviço, programas e pasta são falsos: nada
//! daqui chega ao serviço de verdade nem baixa o `frp` ou o Caddy.
use crate::fake::{OWNER, client, config, spawn_fake, spawn_state};

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hangar_server::connect::CONNECT_PORT;
use hangar_server::connect::binaries::{Digest, Package, fetch, packages};
use hangar_server::connect::config::{render_caddyfile, render_frpc};
use hangar_server::connect::devices::{Refusal, Store};
use hangar_server::connect::state::{self, LinkRequest, Linked};
use hangar_server::connect::supervisor::{Supervisor, kill_leftover};
use hangar_server::routes::AppState;
use serde_json::{Value, json};

const MACHINE_KEY: &str = "chave-de-maquina-de-teste";
const FRPS_TOKEN: &str = "token-do-frps-de-teste";
const PASSWORD: &str = "senha-da-conta-de-teste";
const SERVER_ENV: &str = "HANGAR_CONNECT_TEST_SERVER_DIR";

pub(crate) fn linked() -> Linked {
    Linked {
        service_url: "https://connect.hangar.dev.br".into(),
        account: "acme".into(),
        machine_id: "pc1".into(),
        machine_key: MACHINE_KEY.into(),
        host: "pc1.acme.hangar.dev.br".into(),
        guest_host: "pc1--guest.acme.hangar.dev.br".into(),
        preview_host: "pc1--preview.acme.hangar.dev.br".into(),
        frps_addr: "connect.hangar.dev.br:443".into(),
        frps_token: FRPS_TOKEN.into(),
        service_public_key: STANDARD.encode([7u8; 32]),
        guest_published: false,
        preview_published: false,
    }
}

pub(crate) async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

#[cfg(windows)]
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(format!("{name}.cmd"));
    std::fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
    path
}

#[cfg(unix)]
const LONG_RUN: &str = "exec sleep 300";
#[cfg(windows)]
const LONG_RUN: &str = "ping -n 300 127.0.0.1 >nul";

/// O `frpc` falso só "entra" quando o arquivo `gate` aparece.
#[cfg(unix)]
fn frpc_body(gate: &Path) -> String {
    format!("until [ -e '{}' ]; do sleep 0.05; done\necho '[I] login to server success, get run id [teste]'\n{LONG_RUN}", gate.display())
}

#[cfg(windows)]
fn frpc_body(gate: &Path) -> String {
    format!(":wait\r\nif exist \"{}\" goto logged\r\nping -n 2 127.0.0.1 >nul\r\ngoto wait\r\n:logged\r\necho login to server success\r\n{LONG_RUN}", gate.display())
}

pub(crate) fn gate(dir: &Path) -> PathBuf {
    dir.join("login-gate")
}

fn supervisor(dir: &Path, devices: Arc<Store>, main_port: u16) -> Supervisor {
    let frpc = script(dir, "fake-frpc", &frpc_body(&gate(dir)));
    let caddy = script(dir, "fake-caddy", LONG_RUN);
    Supervisor::new(dir.to_path_buf(), dir.join("logs"), devices, main_port).with_programs(frpc, caddy)
}

/// Sem servidor na frente: a porta conta como aberta por ele.
pub(crate) fn alone(dir: &Path) -> Supervisor {
    let supervisor = supervisor(dir, Arc::new(Store::new(dir.to_path_buf(), None)), 8765);
    supervisor.port_opened();
    supervisor
}

pub(crate) fn read_pid(dir: &Path, name: &str) -> Option<u32> {
    std::fs::read_to_string(dir.join(format!("{name}.pid"))).ok()?.trim().parse().ok()
}

fn has_pid(dir: &Path, name: &str) -> bool {
    read_pid(dir, name).is_some()
}

pub(crate) fn pid_of(dir: &Path, name: &str) -> u32 {
    read_pid(dir, name).unwrap()
}

fn alive(pid: u32) -> bool {
    hangar_server::runtime::process::argv(pid).is_some_and(|argv| !argv.is_empty())
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    unsafe { libc::kill(pid as i32, libc::SIGKILL); }
    #[cfg(windows)]
    std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output().unwrap();
}

#[derive(Default)]
struct Service {
    refuse_login: bool,
    name_taken: bool,
    /// Resposta do registro da máquina; `None` é sucesso.
    machine_refusal: Option<StatusCode>,
    sessions: Mutex<Vec<Value>>,
    accounts: Mutex<Vec<Value>>,
    machines: Mutex<Vec<(String, Value)>>,
}

async fn signup(State(service): State<Arc<Service>>, body: Bytes) -> (StatusCode, String) {
    service.accounts.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
    if service.name_taken {
        return (StatusCode::CONFLICT, json!({"detail": "conta já existe"}).to_string());
    }
    (StatusCode::CREATED, json!({"token": "conta-nova"}).to_string())
}

async fn session(State(service): State<Arc<Service>>, body: Bytes) -> (StatusCode, String) {
    service.sessions.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
    if service.refuse_login {
        return (StatusCode::UNAUTHORIZED, json!({"detail": "recusado"}).to_string());
    }
    (StatusCode::OK, json!({"token": "sessao-de-teste"}).to_string())
}

async fn register(State(service): State<Arc<Service>>, headers: HeaderMap, body: Bytes) -> (StatusCode, String) {
    let bearer = headers[header::AUTHORIZATION].to_str().unwrap().to_owned();
    service.machines.lock().unwrap().push((bearer, serde_json::from_slice(&body).unwrap()));
    if let Some(refusal) = service.machine_refusal {
        return (refusal, json!({"detail": "recusado"}).to_string());
    }
    let machine = linked();
    (StatusCode::OK, json!({"machine_id": machine.machine_id, "machine_key": machine.machine_key, "host": machine.host,
        "guest_host": machine.guest_host, "preview_host": machine.preview_host, "frps_addr": machine.frps_addr,
        "frps_token": machine.frps_token, "service_public_key": machine.service_public_key}).to_string())
}

async fn spawn_service(refuse_login: bool) -> (Arc<Service>, SocketAddr) {
    spawn_service_with(Service { refuse_login, ..Service::default() }).await
}

async fn spawn_service_with(service: Service) -> (Arc<Service>, SocketAddr) {
    let service = Arc::new(service);
    let app = axum::Router::new().route("/v1/sessions", post(session)).route("/v1/accounts", post(signup))
        .route("/v1/machines", post(register)).with_state(service.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (service, addr)
}

struct Machine {
    dir: tempfile::TempDir,
    addr: SocketAddr,
    devices: Arc<Store>,
    supervisor: Arc<Supervisor>,
}

async fn machine_on(main_port: u16, connect: Option<SocketAddr>) -> Machine {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(gate(dir.path()), "").unwrap();
    let (_python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    let devices = Arc::new(Store::new(dir.path().to_path_buf(), None));
    let supervisor = Arc::new(supervisor(dir.path(), devices.clone(), main_port));
    if connect.is_none() && main_port != CONNECT_PORT {
        supervisor.port_opened();
    }
    state.connect_devices = devices.clone();
    state.connect_supervisor = supervisor.clone();
    state.connect = connect;
    Machine { addr: spawn_state(state).await, dir, devices, supervisor }
}

async fn machine() -> Machine {
    machine_on(8765, None).await
}

async fn link(machine: &Machine, service: SocketAddr) -> reqwest::Response {
    client().post(format!("http://{}/api/connect/link", machine.addr)).bearer_auth(OWNER)
        .json(&json!({"service_url": format!("http://{service}"), "account": "acme", "password": PASSWORD, "machine_name": "pc1"}))
        .send().await.unwrap()
}

async fn forget(machine: &Machine, token: &str) -> reqwest::Response {
    client().delete(format!("http://{}/api/connect", machine.addr)).bearer_auth(token).send().await.unwrap()
}

async fn connect_status(machine: &Machine) -> Value {
    let response = client().get(format!("http://{}/api/connect", machine.addr)).bearer_auth(OWNER).send().await.unwrap();
    serde_json::from_str(&response.text().await.unwrap()).unwrap()
}

async fn error_code(response: reqwest::Response) -> (u16, String) {
    let status = response.status().as_u16();
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    (status, body["detail"]["code"].as_str().unwrap().to_owned())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> Digest {
    Digest::Sha256(hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref()))
}

/// Serve `bytes` em `/package` e conta os pedidos.
async fn serve_bytes(bytes: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let app = axum::Router::new().route("/package", get(move || {
        let (bytes, counted) = (bytes.clone(), counted.clone());
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            bytes
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/package"), hits)
}

fn tar_gz(member: &str, content: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
    let mut entry = tar::Header::new_gnu();
    entry.set_size(content.len() as u64);
    entry.set_mode(0o755);
    builder.append_data(&mut entry, member, content).unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

fn zip_of(member: &str, content: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer.start_file(member, zip::write::SimpleFileOptions::default()).unwrap();
    writer.write_all(content).unwrap();
    writer.finish().unwrap().into_inner()
}

fn package(url: String, data: &[u8], member: &str) -> Package {
    Package { version: "teste", url, member: member.to_owned(), file: "frpc".to_owned(), digest: sha256(data) }
}

#[tokio::test]
async fn frpc_config_matches_the_proven_file() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let supervisor = alone(dir);
    supervisor.start(&linked()).await.unwrap();
    let caddyfile = std::fs::read_to_string(dir.join("Caddyfile")).unwrap();
    assert!(caddyfile.contains("\thttp_port 18080\n\thttps_port 18443\n"), "{caddyfile}");
    assert_eq!(std::fs::read_to_string(dir.join("frpc.toml")).unwrap(), format!(r#"serverAddr = "connect.hangar.dev.br"
serverPort = 443
transport.protocol = "wss"
auth.method = "token"
auth.token = "{FRPS_TOKEN}"
loginFailExit = false
metadatas.key = "{MACHINE_KEY}"
transport.heartbeatInterval = 30

[[proxies]]
name = "pc1.acme.hangar.dev.br"
type = "https"
localIP = "127.0.0.1"
localPort = 18443
customDomains = ["pc1.acme.hangar.dev.br"]
"#));
    supervisor.stop().await;
}

#[test]
fn frpc_config_rejects_injection() {
    for bad in ["abcdefghij\"\nserverAddr = \"evil.example", "abcdefghijklmnop\n", "abcdefghijklmnop\"", "", "curta"] {
        let mut token = linked();
        token.frps_token = bad.into();
        let mut key = linked();
        key.machine_key = bad.into();
        let mut host = linked();
        host.host = bad.into();
        let mut addr = linked();
        addr.frps_addr = bad.into();
        for value in [token, key, host, addr] {
            assert_eq!(render_frpc(&value).unwrap_err().code, "connect_service_invalid", "{bad:?}");
            assert_eq!(render_caddyfile(&value, Path::new("/casa")).unwrap_err().code, "connect_service_invalid", "{bad:?}");
        }
    }
    let mut short = linked();
    short.host = "acme.hangar".into();
    assert!(render_frpc(&short).is_err(), "host precisa de dois pontos");
}

#[test]
fn caddyfile_binds_loopback_and_disables_http_challenge() {
    let text = render_caddyfile(&linked(), Path::new("/casa/.hangar/connect")).unwrap();
    assert_eq!(text, "{
\tadmin off
\thttp_port 18080
\thttps_port 18443
\tauto_https disable_redirects
\tstorage file_system \"/casa/.hangar/connect/caddy\"
\tservers {
\t\ttimeouts {
\t\t\tread_header 10s
\t\t\tread_body 1h
\t\t}
\t}
\tlog {
\t\tlevel WARN
\t}
}

pc1.acme.hangar.dev.br {
\tbind 127.0.0.1
\ttls {
\t\tissuer acme {
\t\t\tdisable_http_challenge
\t\t}
\t}
\treverse_proxy 127.0.0.1:8768
}
");
    assert_eq!(render_caddyfile(&linked(), Path::new("/casa/a\"b")).unwrap_err().code, "connect_path_invalid");
}

#[tokio::test]
async fn binary_with_wrong_hash_is_refused() {
    let (url, _hits) = serve_bytes(b"pacote adulterado".to_vec()).await;
    let [frp, caddy] = packages("linux", "amd64").unwrap();
    assert!(matches!(frp.digest, Digest::Sha256(_)));
    assert!(matches!(caddy.digest, Digest::Sha512(_)));
    for mut package in [frp, caddy] {
        package.url = url.clone();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fetch(&package, dir.path()).await.unwrap_err().code, "connect_binary_hash");
        assert!(!dir.path().join("bin").exists(), "nada chega ao disco");
    }
}

#[test]
fn package_names_follow_each_project() {
    let [frp, caddy] = packages("darwin", "arm64").unwrap();
    assert!(frp.url.ends_with("/v0.71.0/frp_0.71.0_darwin_arm64.tar.gz"), "{}", frp.url);
    assert!(caddy.url.ends_with("/v2.11.6/caddy_2.11.6_mac_arm64.tar.gz"), "{}", caddy.url);
    let [frp, caddy] = packages("windows", "amd64").unwrap();
    assert_eq!((frp.member.as_str(), frp.file.as_str()), ("frp_0.71.0_windows_amd64/frpc.exe", "frpc.exe"));
    assert!(caddy.url.ends_with("/caddy_2.11.6_windows_amd64.zip"), "{}", caddy.url);
    assert_eq!(packages("windows", "arm64").err().unwrap().code, "connect_platform_unsupported");
}

#[tokio::test]
async fn package_is_installed_once_per_version() {
    let data = tar_gz("frp_x/frpc", b"programa");
    let (url, hits) = serve_bytes(data.clone()).await;
    let package = package(url, &data, "frp_x/frpc");
    let dir = tempfile::tempdir().unwrap();
    let installed = fetch(&package, dir.path()).await.unwrap();
    assert_eq!(installed, dir.path().join("bin/teste/frpc"));
    assert_eq!(std::fs::read(&installed).unwrap(), b"programa");
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&installed).unwrap().permissions()) & 0o777, 0o755);
    fetch(&package, dir.path()).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn existing_binary_gets_the_execute_permission() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("bin/teste/frpc");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, "programa").unwrap();
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (url, hits) = serve_bytes(Vec::new()).await;
    fetch(&package(url, b"", "frpc"), dir.path()).await.unwrap();
    assert_eq!(std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o755);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn zip_package_is_read_and_missing_member_is_refused() {
    let data = zip_of("frp_x/frpc.exe", b"programa");
    let (url, _hits) = serve_bytes(data.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let missing = fetch(&package(url.clone(), &data, "outro"), dir.path()).await.unwrap_err();
    assert_eq!(missing.code, "connect_binary_missing");
    let installed = fetch(&package(url, &data, "frp_x/frpc.exe"), dir.path()).await.unwrap();
    assert_eq!(std::fs::read(installed).unwrap(), b"programa");
}

#[test]
fn unreadable_state_is_an_error_not_unlinked() {
    let dir = tempfile::tempdir().unwrap();
    assert!(state::read(dir.path()).unwrap().is_none(), "só arquivo ausente é não ligado");
    let unreadable = |dir: &Path| state::read(dir).err().map(|error| error.code);
    std::fs::write(dir.path().join("connect.json"), r#"{"code":"antigo","enabled":true}"#).unwrap();
    assert_eq!(unreadable(dir.path()), Some("connect_state_unreadable"));
    let mut tampered = serde_json::to_value(linked()).unwrap();
    tampered["host"] = json!("x\"\ny.z.w");
    std::fs::write(dir.path().join("connect.json"), tampered.to_string()).unwrap();
    assert_eq!(unreadable(dir.path()), Some("connect_state_unreadable"), "o que vai cru ao TOML é conferido também na volta do disco");
    std::fs::remove_file(dir.path().join("connect.json")).unwrap();
    std::fs::create_dir(dir.path().join("connect.json")).unwrap();
    assert_eq!(unreadable(dir.path()), Some("connect_state_unreadable"), "falha de leitura também");
}

#[test]
fn unreadable_devices_reach_status() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("devices.json")).unwrap();
    let error = alone(dir.path()).status().error.unwrap();
    assert!(error.contains("aparelhos"), "{error}");
}

#[tokio::test]
async fn unreadable_state_reaches_status_and_a_new_link_keeps_the_old_file() {
    let (_, refusing) = spawn_service(true).await;
    let (_, accepting) = spawn_service(false).await;
    let request = |service: SocketAddr| LinkRequest { service_url: format!("http://{service}"), account: "acme".into(),
        password: PASSWORD.into(), machine_name: "pc1".into(), create_account: false };
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let supervisor = alone(dir);
    std::fs::write(gate(dir), "").unwrap();
    std::fs::write(dir.join("connect.json"), "{quebrado").unwrap();

    supervisor.resume().await;
    let status = supervisor.status();
    assert!(status.error.as_deref().is_some_and(|error| error.contains("não foi lida")), "{status:?}");
    assert!(!status.running);
    assert!(supervisor.read_linked().await.is_err());

    // Registro recusado não tira o arquivo do lugar: pode ser um bom que falhou por um instante.
    assert!(supervisor.link(request(refusing)).await.is_err());
    assert_eq!(std::fs::read_to_string(dir.join("connect.json")).unwrap(), "{quebrado");

    supervisor.link(request(accepting)).await.unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("connect.json.bad")).unwrap(), "{quebrado");
    assert!(state::read(dir).unwrap().is_some());
    assert_eq!(supervisor.status().error, None);
    supervisor.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn refused_address_reaches_status_until_the_next_login() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let frpc = script(dir, "fake-frpc", &format!("echo '[I] login to server success, get run id [teste]'
printf '\\033[1;33m[W] [pc1.acme.hangar.dev.br] start error: dominio recusado\\033[0m\\n'
until [ -e '{}' ]; do sleep 0.05; done
echo '[I] login to server success, get run id [outra]'
{LONG_RUN}", gate(dir).display()));
    let caddy = script(dir, "fake-caddy", LONG_RUN);
    let supervisor = Supervisor::new(dir.to_path_buf(), dir.join("logs"), Arc::new(Store::new(dir.to_path_buf(), None)), 8765).with_programs(frpc, caddy);
    supervisor.port_opened();
    supervisor.start(&linked()).await.unwrap();

    until("a recusa no status", || supervisor.status().error.is_some()).await;
    let error = supervisor.status().error.unwrap();
    assert!(error.ends_with("dominio recusado"), "{error}");
    until("Caddy de pé", || has_pid(dir, "caddy")).await;
    assert!(!supervisor.status().enabled, "login sem endereço não é ligado");

    std::fs::write(gate(dir), "").unwrap();
    until("Connect de pé", || supervisor.status().enabled).await;
    assert_eq!(supervisor.status().error, None);
    supervisor.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_frpc_log_reaches_status() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let supervisor = alone(dir);
    supervisor.start(&linked()).await.unwrap();
    until("o frpc não nasceu", || has_pid(dir, "frpc")).await;
    std::fs::remove_file(dir.join("logs/connect-frpc.log")).unwrap();
    until("a falha de leitura no status", || supervisor.status().error.is_some_and(|error| error.contains("não consegui ler"))).await;
    supervisor.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn child_logs_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let supervisor = alone(dir);
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    let old = dir.join("logs/connect-frpc.log");
    std::fs::write(&old, "de antes\n").unwrap();
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(gate(dir), "").unwrap();
    supervisor.start(&linked()).await.unwrap();
    until("Caddy de pé", || has_pid(dir, "caddy")).await;
    for name in ["connect-frpc.log", "connect-caddy.log"] {
        assert_eq!(std::fs::metadata(dir.join("logs").join(name)).unwrap().permissions().mode() & 0o777, 0o600, "{name}");
    }
    supervisor.stop().await;
}

#[tokio::test]
async fn enabled_true_only_when_both_children_run() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let supervisor = alone(dir);
    let idle = supervisor.status();
    assert!(!idle.enabled && !idle.running);

    supervisor.start(&linked()).await.unwrap();
    until("o frpc não nasceu", || has_pid(dir, "frpc")).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let waiting = supervisor.status();
    assert!(waiting.running && !waiting.enabled, "só o frpc de pé não é ligado");
    assert_eq!(waiting.host.as_deref(), Some("pc1.acme.hangar.dev.br"));
    assert!(!dir.join("caddy.pid").exists(), "o Caddy espera o login do frpc");

    std::fs::write(gate(dir), "").unwrap();
    until("os dois não subiram", || supervisor.status().enabled).await;

    let (frpc, caddy) = (pid_of(dir, "frpc"), pid_of(dir, "caddy"));
    kill_pid(caddy);
    until("a queda do Caddy não apareceu", || !supervisor.status().enabled).await;
    assert!(supervisor.status().running, "a supervisão segue");

    supervisor.stop().await;
    let stopped = supervisor.status();
    assert!(!stopped.enabled && !stopped.running);
    until("o frpc ficou de pé depois do stop", || !alive(frpc)).await;
}

#[tokio::test]
async fn caddy_stops_when_the_port_is_lost_and_comes_back_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    std::fs::write(gate(dir), "").unwrap();
    let supervisor = alone(dir);
    supervisor.start(&linked()).await.unwrap();
    until("os dois não subiram", || supervisor.status().enabled).await;
    let caddy = pid_of(dir, "caddy");

    supervisor.port_failed("porta perdida".to_owned());
    until("o Caddy ficou de pé sem a porta", || !supervisor.status().enabled).await;
    assert!(!alive(caddy));
    let lost = supervisor.status();
    assert!(lost.running);
    assert_eq!(lost.error.as_deref(), Some("porta perdida"));

    supervisor.port_opened();
    until("o Caddy não voltou com a porta", || supervisor.status().enabled).await;
    assert_ne!(pid_of(dir, "caddy"), caddy);
    supervisor.stop().await;
}

#[tokio::test]
async fn caddy_waits_for_a_new_login_after_frpc_falls() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    std::fs::write(gate(dir), "").unwrap();
    let supervisor = alone(dir);
    supervisor.start(&linked()).await.unwrap();
    until("os dois não subiram", || supervisor.status().enabled).await;
    let (frpc, caddy) = (pid_of(dir, "frpc"), pid_of(dir, "caddy"));

    std::fs::remove_file(gate(dir)).unwrap();
    kill_pid(frpc);
    until("o frpc não voltou", || read_pid(dir, "frpc").is_some_and(|pid| pid != frpc)).await;
    kill_pid(caddy);
    // Passa da espera do Caddy depois da primeira queda.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(pid_of(dir, "caddy"), caddy, "sem login novo o Caddy não nasce");
    assert!(!supervisor.status().enabled);

    std::fs::write(gate(dir), "").unwrap();
    until("o Caddy não voltou com o login", || supervisor.status().enabled).await;
    assert_ne!(pid_of(dir, "caddy"), caddy);
    supervisor.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn simultaneous_start_leaves_one_of_each() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    std::fs::write(gate(dir), "").unwrap();
    let births = dir.join("births");
    let frpc = script(dir, "counted-frpc", &format!("echo $$ >> '{}'\n{}", births.display(), frpc_body(&gate(dir))));
    let caddy = script(dir, "fake-caddy", LONG_RUN);
    let supervisor = Supervisor::new(dir.to_path_buf(), dir.join("logs"), Arc::new(Store::new(dir.to_path_buf(), None)), 8765)
        .with_programs(frpc, caddy);
    supervisor.port_opened();
    let machine = linked();
    let (first, second) = tokio::join!(supervisor.start(&machine), supervisor.start(&machine));
    first.unwrap();
    second.unwrap();
    until("os dois não subiram", || supervisor.status().enabled).await;
    let text = std::fs::read_to_string(&births).unwrap();
    let living: Vec<u32> = text.lines().map(|line| line.parse().unwrap()).filter(|pid| alive(*pid)).collect();
    assert_eq!(living, vec![pid_of(dir, "frpc")], "{text}");
    supervisor.stop().await;
}

#[tokio::test]
async fn child_log_is_rotated_when_it_starts() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    std::fs::write(dir.join("logs/connect-frpc.log"), vec![b'x'; (4 << 20) + 1]).unwrap();
    let supervisor = alone(dir);
    supervisor.start(&linked()).await.unwrap();
    until("o frpc não nasceu", || has_pid(dir, "frpc")).await;
    assert!(dir.join("logs/connect-frpc.log.1").exists());
    supervisor.stop().await;
}

#[tokio::test]
async fn repeated_crash_shows_up_in_error() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let (frpc, caddy) = (script(dir, "crash", "exit 3"), script(dir, "fake-caddy", LONG_RUN));
    let supervisor = Supervisor::new(dir.to_path_buf(), dir.join("logs"), Arc::new(Store::new(dir.to_path_buf(), None)), 8765)
        .with_programs(frpc, caddy);
    supervisor.start(&linked()).await.unwrap();
    until("as quedas não apareceram", || supervisor.status().error.is_some_and(|error| error.contains("frpc caiu"))).await;
    assert!(!supervisor.status().enabled);
    supervisor.stop().await;
}

async fn link_creating(machine: &Machine, service: SocketAddr) -> reqwest::Response {
    client().post(format!("http://{}/api/connect/link", machine.addr)).bearer_auth(OWNER)
        .json(&json!({"service_url": format!("http://{service}"), "account": "acme", "password": PASSWORD,
            "machine_name": "pc1", "create_account": true}))
        .send().await.unwrap()
}

#[tokio::test]
async fn link_can_create_the_account_first() {
    let (service, address) = spawn_service(false).await;
    let machine = machine().await;
    let response = link_creating(&machine, address).await;
    assert_eq!(response.status(), 200);
    assert_eq!(*service.accounts.lock().unwrap(), vec![json!({"name": "acme", "password": PASSWORD})]);
    assert!(service.sessions.lock().unwrap().is_empty(), "conta nova não passa pelo login");
    assert_eq!(*service.machines.lock().unwrap(), vec![("Bearer conta-nova".to_owned(), json!({"name": "pc1"}))]);
    machine.supervisor.stop().await;
}

#[tokio::test]
async fn link_creating_a_taken_name_says_so() {
    let (service, address) = spawn_service_with(Service { name_taken: true, ..Service::default() }).await;
    let machine = machine().await;
    let response = link_creating(&machine, address).await;
    assert_eq!(response.status(), 409);
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(body["detail"]["code"], "connect_account_taken", "{body}");
    assert!(service.machines.lock().unwrap().is_empty());
    assert!(!machine.dir.path().join("connect.json").exists());
}

async fn link_code(response: reqwest::Response) -> (u16, String) {
    let status = response.status().as_u16();
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    (status, body["detail"]["code"].as_str().unwrap_or_default().to_owned())
}

#[tokio::test]
async fn a_machine_conflict_is_not_a_taken_account_name() {
    let (_, address) = spawn_service_with(Service { machine_refusal: Some(StatusCode::CONFLICT), ..Service::default() }).await;
    let machine = machine().await;
    assert_eq!(link_code(link(&machine, address).await).await, (409, "connect_machine_conflict".to_owned()));
}

#[tokio::test]
async fn account_created_but_machine_refused_says_to_sign_in() {
    let (service, address) = spawn_service_with(Service { machine_refusal: Some(StatusCode::BAD_GATEWAY), ..Service::default() }).await;
    let machine = machine().await;
    let (_, code) = link_code(link_creating(&machine, address).await).await;
    assert_eq!(code, "connect_account_created_link_failed", "a conta existe: tentar criar de novo daria nome já usado");
    assert_eq!(service.accounts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn link_saves_the_identity_and_never_the_password() {
    let (service, address) = spawn_service(false).await;
    let machine = machine().await;
    let response = link(&machine, address).await;
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(!body.contains(MACHINE_KEY) && !body.contains(FRPS_TOKEN), "{body}");
    assert_eq!(service.sessions.lock().unwrap()[0], json!({"name": "acme", "password": PASSWORD}));
    assert_eq!(*service.machines.lock().unwrap(), vec![("Bearer sessao-de-teste".to_owned(), json!({"name": "pc1"}))]);

    let dir = machine.dir.path();
    let saved = std::fs::read_to_string(dir.join("connect.json")).unwrap();
    assert!(saved.contains(MACHINE_KEY) && !saved.contains(PASSWORD));
    let frpc = std::fs::read_to_string(dir.join("frpc.toml")).unwrap();
    assert!(frpc.contains(&format!("metadatas.key = \"{MACHINE_KEY}\"")) && frpc.contains("serverPort = 443"));
    #[cfg(unix)]
    for name in ["connect.json", "frpc.toml"] {
        let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(dir.join(name)).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o600, "{name}");
    }
    assert!(matches!(machine.devices.redeem("{}", "", "x"), Err(Refusal::Ticket)), "a chave do serviço chegou à loja");

    let supervisor = machine.supervisor.clone();
    until("os dois não subiram", || supervisor.status().enabled).await;
    let status = connect_status(&machine).await;
    assert_eq!(status, json!({"enabled": true, "running": true, "host": "pc1.acme.hangar.dev.br", "error": null}));
    let reach = client().get(format!("http://{}/api/alcance", machine.addr)).bearer_auth(OWNER).send().await.unwrap();
    let reach: Value = serde_json::from_str(&reach.text().await.unwrap()).unwrap();
    assert_eq!(reach["connect"], json!("https://pc1.acme.hangar.dev.br"));
    assert!(reach["enderecos"].as_array().unwrap().iter().all(|row| row["tipo"] != "connect"), "tipo novo derruba o parse_reach do nativo");

    let again = link(&machine, address).await;
    assert_eq!(error_code(again).await, (409, "connect_already_linked".to_owned()));
    assert_eq!(service.machines.lock().unwrap().len(), 1, "religar trocaria a chave no serviço");
    machine.supervisor.stop().await;
}

#[tokio::test]
async fn link_comes_back_after_a_restart() {
    let (_service, address) = spawn_service(false).await;
    let machine = machine().await;
    assert_eq!(link(&machine, address).await.status(), 200);
    machine.supervisor.stop().await;

    let dir = machine.dir.path();
    let devices = Arc::new(Store::new(dir.to_path_buf(), None));
    let next = supervisor(dir, devices.clone(), 8765);
    next.port_opened();
    next.resume().await;
    assert!(matches!(devices.redeem("{}", "", "x"), Err(Refusal::Ticket)));
    until("os dois não voltaram", || next.status().enabled).await;
    next.stop().await;
}

#[tokio::test]
async fn link_with_supervision_stopped_starts_again_without_registering() {
    let (service, address) = spawn_service(false).await;
    let machine = machine().await;
    assert_eq!(link(&machine, address).await.status(), 200);
    machine.supervisor.stop().await;

    let other = client().post(format!("http://{}/api/connect/link", machine.addr)).bearer_auth(OWNER)
        .json(&json!({"service_url": format!("http://{address}"), "account": "acme", "password": PASSWORD, "machine_name": "pc2"}))
        .send().await.unwrap();
    assert_eq!(error_code(other).await, (409, "connect_already_linked".to_owned()));

    assert_eq!(link(&machine, address).await.status(), 200);
    assert_eq!(service.machines.lock().unwrap().len(), 1, "registrar de novo trocaria a chave");
    let supervisor = machine.supervisor.clone();
    until("os dois não voltaram", || supervisor.status().enabled).await;
    machine.supervisor.stop().await;
}

#[tokio::test]
async fn link_refuses_what_it_must_not_send() {
    let (service, address) = spawn_service(true).await;
    let machine = machine().await;
    let (status, code) = error_code(link(&machine, address).await).await;
    assert_eq!((status, code.as_str()), (401, "connect_login_refused"));
    assert!(!machine.dir.path().join("connect.json").exists());

    let post = |body: Value| client().post(format!("http://{}/api/connect/link", machine.addr)).bearer_auth(OWNER).json(&body).send();
    let before = service.sessions.lock().unwrap().len();
    let clear = post(json!({"service_url": "http://servico.exemplo", "account": "acme", "password": PASSWORD, "machine_name": "pc1"})).await.unwrap();
    assert_eq!(error_code(clear).await, (400, "connect_service_url_invalid".to_owned()));
    let name = post(json!({"service_url": format!("http://{address}"), "account": "acme", "password": PASSWORD, "machine_name": "pc1\n"})).await.unwrap();
    assert_eq!(error_code(name).await, (400, "connect_name_invalid".to_owned()));
    assert_eq!(service.sessions.lock().unwrap().len(), before, "a senha não saiu");
}

#[tokio::test]
async fn link_is_refused_when_the_main_port_is_the_connect_port() {
    let (service, address) = spawn_service(false).await;
    let machine = machine_on(CONNECT_PORT, None).await;
    assert_eq!(error_code(link(&machine, address).await).await, (409, "connect_port_is_main".to_owned()));
    assert!(service.sessions.lock().unwrap().is_empty());
    assert!(connect_status(&machine).await["error"].as_str().unwrap().contains("CP_PORT=8768"));
}

#[tokio::test]
async fn caddy_is_never_born_while_another_process_holds_the_port() {
    let (_service, address) = spawn_service(false).await;
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let machine = machine_on(8765, Some(busy.local_addr().unwrap())).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !connect_status(&machine).await["error"].as_str().is_some_and(|error| error.contains("indisponível")) {
        assert!(Instant::now() < deadline, "o motivo do bind não apareceu");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(link(&machine, address).await.status(), 200);
    let dir = machine.dir.path();
    let log = dir.join("logs/connect-frpc.log");
    until("o frpc não entrou", || std::fs::read_to_string(&log).is_ok_and(|text| text.contains("login to server success"))).await;
    // Passa do tique que leria o login.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!has_pid(dir, "caddy"), "com a porta de outro, o Caddy publicaria esse processo na internet");
    let waiting = connect_status(&machine).await;
    assert_eq!((&waiting["enabled"], &waiting["running"]), (&json!(false), &json!(true)));
    assert!(waiting["error"].as_str().unwrap().contains("indisponível"));

    drop(busy);
    let supervisor = machine.supervisor.clone();
    until("o Caddy não subiu com a porta", || supervisor.status().enabled).await;
    assert!(connect_status(&machine).await["error"].is_null(), "o motivo ficou depois da porta soltar");
    machine.supervisor.stop().await;
}

#[tokio::test]
async fn device_cannot_link_or_forget() {
    let (service, address) = spawn_service(false).await;
    let machine = machine().await;
    let token = machine.devices.enroll("celular").unwrap().1;
    let body = json!({"service_url": format!("http://{address}"), "account": "acme", "password": PASSWORD, "machine_name": "pc1"});
    let linked = client().post(format!("http://{}/api/connect/link", machine.addr)).bearer_auth(&token).json(&body).send().await.unwrap();
    assert_eq!(error_code(linked).await, (403, "connect_owner_secret_refused".to_owned()));
    assert_eq!(error_code(forget(&machine, &token).await).await, (403, "connect_owner_secret_refused".to_owned()));
    assert!(service.sessions.lock().unwrap().is_empty());
    let read = client().get(format!("http://{}/api/connect", machine.addr)).bearer_auth(&token).send().await.unwrap();
    assert_eq!(read.status(), 200, "ler o estado o aparelho pode");
    let anonymous = client().get(format!("http://{}/api/connect", machine.addr)).send().await.unwrap();
    assert_eq!(anonymous.status(), 401);
}

#[tokio::test]
async fn forget_removes_the_tls_key() {
    let (_service, address) = spawn_service(false).await;
    let machine = machine().await;
    assert_eq!(link(&machine, address).await.status(), 200);
    let supervisor = machine.supervisor.clone();
    until("os dois não subiram", || supervisor.status().enabled).await;
    let dir = machine.dir.path();
    let (frpc, caddy) = (pid_of(dir, "frpc"), pid_of(dir, "caddy"));
    let key = dir.join("caddy/certificates/acme/pc1.acme.hangar.dev.br/pc1.acme.hangar.dev.br.key");
    std::fs::create_dir_all(key.parent().unwrap()).unwrap();
    std::fs::write(&key, "chave tls").unwrap();

    let response = forget(&machine, OWNER).await;
    assert_eq!(response.status(), 200);
    assert!(!dir.join("caddy").exists());
    for name in ["connect.json", "frpc.toml", "Caddyfile", "frpc.pid", "caddy.pid"] {
        assert!(!dir.join(name).exists(), "{name}");
    }
    assert!(!alive(frpc) && !alive(caddy));
    assert_eq!(connect_status(&machine).await, json!({"enabled": false, "running": false, "host": null, "error": null}));
    assert!(matches!(machine.devices.redeem("{}", "", "x"), Err(Refusal::NotLinked)));
}

#[tokio::test]
async fn forget_keeps_device_credentials_working_on_8765() {
    let (_service, address) = spawn_service(false).await;
    let machine = machine().await;
    let token = machine.devices.enroll("celular").unwrap().1;
    assert_eq!(link(&machine, address).await.status(), 200);
    assert_eq!(forget(&machine, OWNER).await.status(), 200);
    assert!(machine.dir.path().join("devices.json").exists());
    let listed = client().get(format!("http://{}/api/connect/devices", machine.addr)).bearer_auth(&token).send().await.unwrap();
    assert_eq!(listed.status(), 200);
    assert!(listed.text().await.unwrap().contains("celular"));
}

fn long_runner() -> (PathBuf, Vec<&'static str>) {
    if cfg!(windows) {
        (PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join("ping.exe"), vec!["-n", "60", "127.0.0.1"])
    } else {
        (PathBuf::from("/bin/sleep"), vec!["60"])
    }
}

/// Em grupo próprio, como o filho de verdade. Cópia recém-gravada pode dar `ETXTBSY` se outro teste
/// estiver no meio de um fork.
fn spawn_copy(path: &Path, args: &[&str]) -> std::process::Child {
    let mut command = std::process::Command::new(path);
    command.args(args);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match command.spawn() {
            Err(error) if cfg!(unix) && error.raw_os_error() == Some(26) && Instant::now() < deadline =>
                std::thread::sleep(Duration::from_millis(20)),
            spawned => return spawned.unwrap(),
        }
    }
}

#[tokio::test]
async fn leftover_is_killed_only_when_the_executable_is_ours() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let (runner, args) = long_runner();
    let file = format!("frpc{}", std::env::consts::EXE_SUFFIX);
    let (own_copy, foreign_copy) = (dir.join("bin/0.71.0").join(&file), dir.join("outro/0.71.0").join(&file));
    for copy in [&own_copy, &foreign_copy] {
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::copy(&runner, copy).unwrap();
    }
    let pid_path = dir.join("frpc.pid");

    let mut foreign = spawn_copy(&foreign_copy, &args);
    std::fs::write(&pid_path, foreign.id().to_string()).unwrap();
    kill_leftover(dir, "frpc").await.unwrap();
    assert!(foreign.try_wait().unwrap().is_none(), "pid reaproveitado nunca morre");
    foreign.kill().unwrap();
    foreign.wait().unwrap();

    let mut own = spawn_copy(&own_copy, &args);
    std::fs::write(&pid_path, own.id().to_string()).unwrap();
    kill_leftover(dir, "frpc").await.unwrap();
    assert!(!own.wait().unwrap().success(), "o que sobrou de uma vida anterior morre");
}

/// O "servidor" de `children_die_with_the_server`: só faz algo chamado por ele.
#[tokio::test]
#[ignore = "processo filho de children_die_with_the_server"]
async fn server_process_for_children_die() {
    let Some(dir) = std::env::var_os(SERVER_ENV) else { return };
    let supervisor = alone(Path::new(&dir));
    supervisor.start(&linked()).await.unwrap();
    std::future::pending::<()>().await;
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn children_die_with_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    std::fs::write(gate(dir), "").unwrap();
    let mut server = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "connect_supervisor::server_process_for_children_die", "--ignored", "--nocapture"])
        .env(SERVER_ENV, dir).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().unwrap();
    until("os filhos não nasceram", || has_pid(dir, "frpc") && has_pid(dir, "caddy")).await;
    let (frpc, caddy) = (pid_of(dir, "frpc"), pid_of(dir, "caddy"));
    assert!(alive(frpc) && alive(caddy));

    server.kill().unwrap();
    server.wait().unwrap();
    until("o frpc sobreviveu ao servidor", || !alive(frpc)).await;
    until("o Caddy sobreviveu ao servidor", || !alive(caddy)).await;
}
