//! O Connect como caminho a mais das máquinas que já estão no `peers.json`. Corre por evento (Connect
//! ligado aqui, lista da conta lida, máquina gravada pelo dono), nunca por varredura periódica.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::{Value, json};

use super::Service;
use super::network::revoke_device;
use super::store::Book;
use crate::connect::peers::{AccountMachine, account_machines, on_connect_host};
use crate::groups::peers::{PeerError, client, request};
use crate::routes::AppState;

/// Máquina que falhou espera isto antes do próximo evento tentar de novo: abrir a tela do Connect não
/// repete a chamada nem enche o diário.
const COOLDOWN: Duration = Duration::from_secs(600);

/// Connect ligado nesta máquina ou máquina gravada pelo dono: a lista da conta só é lida se houver o que fazer.
pub fn spawn(st: &Arc<AppState>) {
    tokio::spawn(run(st.clone(), None));
}

/// A tela acabou de ler a lista da conta.
pub fn spawn_with(st: &Arc<AppState>, machines: Vec<AccountMachine>) {
    tokio::spawn(run(st.clone(), Some(machines)));
}

async fn run(st: Arc<AppState>, machines: Option<Vec<AccountMachine>>) {
    let service = match Service::from_state(&st) {
        Ok(service) => service,
        Err(failure) => return report(&st, "", failure.code, "a configuração das máquinas não foi lida"),
    };
    // Sem ligação aqui não há conta para conferir o endereço de lá.
    let Ok(Some(linked)) = st.connect_supervisor.read_linked().await else { return };
    let link = format!("https://{}", linked.host);
    let (store, read_link) = (service.store.clone(), link.clone());
    let mut ids = tokio::task::spawn_blocking(move || candidates(&store.tolerant(), Some(&read_link))).await.unwrap_or_default();
    let now = Instant::now();
    ids.retain(|id| !cooling(&(service.store.path.clone(), id.clone()), now));
    if ids.is_empty() {
        return;
    }
    let machines = match machines {
        Some(machines) => machines,
        None => match account_machines(&linked).await {
            Ok(machines) => machines,
            Err(failure) => return report(&st, "", failure.code, "a lista de máquinas da conta não foi lida"),
        },
    };
    let machines = Arc::new(machines);
    for id in ids {
        let Some(slot) = Slot::claim(&service.store.path, &id) else { continue };
        let (st, service, machines, link) = (st.clone(), service.clone(), machines.clone(), link.clone());
        tokio::spawn(async move {
            match enroll(&service, &id, &link, &machines).await {
                Ok(()) => forget(&slot.0),
                Err((code, detail)) => {
                    note_failure(slot.0.clone(), Instant::now());
                    tracing::warn!(code = "connect.peer_route_failed", reason = code, peer = %id, "o Connect não virou caminho de '{id}': {detail}");
                    report(&st, &id, code, "o Connect não virou caminho da máquina");
                }
            }
            drop(slot);
        });
    }
}

/// Entradas salvas pelo dono, ativas, ainda sem o Connect e sem recusa. Com a marca `connect` ou endereço já
/// no Connect não há caminho a somar.
fn candidates(book: &Book, link: Option<&str>) -> Vec<String> {
    book.into_iter().filter(|(_, entry)| candidate(entry, link)).map(|(id, _)| id.clone()).collect()
}

fn candidate(entry: &Value, link: Option<&str>) -> bool {
    let filled = |value: &Value| value.as_str().is_some_and(|text| !text.is_empty());
    entry.is_object() && entry["enabled"] != false && entry["connect"] != true
        && entry["connect_refused"] != true && entry.get("connect_url").is_none() && filled(&entry["token"])
        && entry["base_url"].as_str().is_some_and(|url| !url.is_empty() && !on_connect_host(link, url))
}

type Key = (PathBuf, String);

fn failures() -> &'static Mutex<HashMap<Key, Instant>> {
    static FAILED: OnceLock<Mutex<HashMap<Key, Instant>>> = OnceLock::new();
    FAILED.get_or_init(Mutex::default)
}

fn cooling(key: &Key, now: Instant) -> bool {
    failures().lock().unwrap_or_else(|e| e.into_inner()).get(key).is_some_and(|at| now.saturating_duration_since(*at) < COOLDOWN)
}

fn note_failure(key: Key, now: Instant) {
    failures().lock().unwrap_or_else(|e| e.into_inner()).insert(key, now);
}

fn forget(key: &Key) {
    failures().lock().unwrap_or_else(|e| e.into_inner()).remove(key);
}

/// O dono acabou de gravar a máquina: a espera da falha anterior não vale para o que ele corrigiu.
pub fn forget_failure(path: &Path, id: &str) {
    forget(&(path.to_path_buf(), id.to_owned()));
}

/// Uma emissão por máquina e por arquivo: dois eventos juntos emitiriam duas credenciais lá.
struct Slot(Key);

fn running() -> &'static Mutex<HashSet<(PathBuf, String)>> {
    static RUNNING: OnceLock<Mutex<HashSet<(PathBuf, String)>>> = OnceLock::new();
    RUNNING.get_or_init(Mutex::default)
}

impl Slot {
    fn claim(path: &std::path::Path, id: &str) -> Option<Self> {
        let key = (path.to_path_buf(), id.to_owned());
        running().lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone()).then(|| Self(key))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        running().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

type Failure = (&'static str, String);

async fn enroll(service: &Service, id: &str, link: &str, machines: &[AccountMachine]) -> Result<(), Failure> {
    // Relida dentro da vez desta máquina: outro evento pode ter acabado de gravar o caminho ou a recusa.
    let (store, key) = (service.store.clone(), id.to_owned());
    let entry = tokio::task::spawn_blocking(move || store.tolerant().get(&key).cloned()).await.ok().flatten().unwrap_or_default();
    if !candidate(&entry, Some(link)) {
        return Ok(());
    }
    let (Some(base), Some(owner)) = (entry["base_url"].as_str(), entry["token"].as_str()) else { return Ok(()) };
    let (base, owner) = (base.trim_end_matches('/').to_owned(), owner.to_owned());
    // Sem o Connect e sem redirect: as duas chamadas levam a senha do dono, que só vai pelo endereço salvo.
    let http = client(crate::http_client().redirect(reqwest::redirect::Policy::none()))
        .map_err(|e| ("connect_peer_unreachable", format!("cliente HTTP indisponível: {e}")))?;
    let failed = |e: PeerError| (code_of(&e), e.text(id));
    let call = |method: Method, path: &str, body: Option<Value>| {
        let (http, url, owner) = (http.clone(), format!("{base}{path}"), owner.clone());
        async move { request(&http, id, &url, Some(&owner), method, body.as_ref()).await.map(|(_, value)| value.unwrap_or_default()) }
    };
    let state = call(Method::GET, "/api/connect", None).await.map_err(failed)?;
    // Fora do Connect lá não é falha: o próximo evento confere de novo.
    let Some(host) = state["host"].as_str().filter(|host| !host.is_empty()) else { return Ok(()) };
    // O endereço gravado é o da lista da conta, nunca o que a outra máquina disse.
    let Some(machine) = machines.iter().find(|machine| machine.host.eq_ignore_ascii_case(host)) else {
        return Err(("connect_peer_host_outside_account", format!("'{id}' diz estar no Connect em {host}, que não é desta conta")));
    };
    let url = format!("https://{}", machine.host);
    let body = json!({"device_name": format!("máquina {}", service.identity())});
    let issued = call(Method::POST, "/api/connect/devices/self", Some(body)).await.map_err(failed)?;
    let (Some(device), Some(token)) = (issued["device_id"].as_str(), issued["token"].as_str()) else {
        return Err(("connect_peer_invalid_reply", format!("'{id}' não devolveu a credencial de aparelho")));
    };
    let (store, key, device_id, saved_token, saved_url) = (service.store.clone(), id.to_owned(), device.to_owned(), token.to_owned(), url.clone());
    let saved = tokio::task::spawn_blocking(move || store.set_connect_route(&key, &saved_url, &saved_token, &device_id)).await;
    let failure = match saved {
        Ok(Ok(previous)) => {
            // Caminho que já estava lá foi substituído: o aparelho dele se revoga pelo próprio Connect.
            if let Some(old) = previous {
                revoke_device(&old.id, None, Some((old.url, old.token)), &old.device).await;
            }
            return Ok(());
        }
        Ok(Err(e)) => (e.code, e.msg),
        Err(_) => ("peers_response_failed", "a gravação foi interrompida".to_owned()),
    };
    // Sem gravar aqui (entrada apagada ou recusada no meio), a credencial emitida lá ficaria valendo sem dono.
    // O endereço e as duas credenciais já estão à mão: a entrada pode nem existir mais.
    revoke_device(id, Some((base, owner)), Some((url, token.to_owned())), device).await;
    Err(failure)
}

fn code_of(error: &PeerError) -> &'static str {
    match error {
        PeerError::Transport(_) => "connect_peer_unreachable",
        PeerError::Refused { status: 401, .. } => "connect_peer_password_refused",
        // Sem a rota: Hangar de lá anterior ao Connect.
        PeerError::Refused { status: 404, .. } => "connect_peer_old_version",
        _ => "connect_peer_refused",
    }
}

fn report(st: &AppState, peer: &str, code: &str, reason: &'static str) {
    st.diag.report_with("connect.peer_route_failed", "", code, reason, json!({"peer": peer}));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(value: Value) -> Book {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        std::fs::write(&path, value.to_string()).unwrap();
        super::super::store::Store::new(path).read().unwrap()
    }

    #[test]
    fn only_saved_entries_without_a_route_are_candidates() {
        let peers = book(json!({
            "tailscale": {"base_url": "https://note.tail.ts.net", "token": "senha"},
            "off": {"base_url": "https://off.tail.ts.net", "token": "senha", "enabled": false},
            "device": {"base_url": "https://b.acme.hangar.dev.br", "token": "aparelho", "connect": true},
            "owner_on_connect": {"base_url": "https://c.acme.hangar.dev.br", "token": "senha"},
            "routed": {"base_url": "https://r.tail.ts.net", "token": "senha", "connect_url": "https://r.acme.hangar.dev.br",
                "connect_token": "aparelho", "connect_device": "abc"},
            "refused": {"base_url": "https://x.tail.ts.net", "token": "senha", "connect_refused": true},
            "app": {"app": {"nome": "celular"}},
        }));
        assert_eq!(candidates(&peers, Some("https://a.acme.hangar.dev.br")), vec!["tailscale".to_owned()]);
    }

    /// `/api/peers` diz o caminho só da máquina que já atendeu uma chamada de verdade.
    #[tokio::test]
    async fn the_peer_list_shows_the_route_of_the_last_answered_call() {
        let app = axum::Router::new().route("/x", axum::routing::any(|| async { "{}" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let connect = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        std::fs::write(&path, json!({
            "note": {"base_url": format!("http://{closed}"), "token": "senha", "connect_url": format!("http://{connect}"),
                "connect_token": "aparelho", "connect_device": "abc"},
            "casa": {"base_url": "http://100.64.0.9:8765", "token": "senha"},
        }).to_string()).unwrap();
        use crate::groups::peers::{PeerBook, PeerClient};
        let peers = PeerClient::new(PeerBook::new(Some(path.clone()))).with_connect_route();
        peers.call("note", Method::GET, "/x", None).await.unwrap();
        let mut rows = super::super::store::Store::new(path).list();
        super::super::with_routes(&peers, &mut rows);
        assert_eq!(rows[0]["id"], "casa");
        assert!(rows[0].get("route").is_none(), "sem chamada desde a subida, o caminho fica de fora");
        assert_eq!(rows[1]["route"], "connect");
        assert!(rows[1].get("connect_token").is_none() && !rows.to_string().contains("aparelho"), "a credencial não sai na lista");
    }

    /// Máquina que falhou fica 10 min sem nova tentativa, salvo o dono gravá-la de novo.
    #[test]
    fn a_failed_machine_waits_the_cooldown_unless_the_owner_saves_it() {
        let path = std::path::Path::new("/tmp/peers-cooldown-test.json");
        let key = (path.to_path_buf(), "note".to_owned());
        let failed_at = Instant::now();
        assert!(!cooling(&key, failed_at));
        note_failure(key.clone(), failed_at);
        assert!(cooling(&key, failed_at + Duration::from_secs(60)));
        assert!(!cooling(&key, failed_at + COOLDOWN + Duration::from_secs(1)));
        assert!(!cooling(&(path.to_path_buf(), "outra".to_owned()), failed_at));
        forget_failure(path, "note");
        assert!(!cooling(&key, failed_at));
    }

    #[test]
    fn a_second_claim_waits_for_the_first_to_end() {
        let path = std::path::Path::new("/tmp/peers-slot-test.json");
        let first = Slot::claim(path, "note").unwrap();
        assert!(Slot::claim(path, "note").is_none());
        assert!(Slot::claim(path, "outra").is_some());
        drop(first);
        assert!(Slot::claim(path, "note").is_some());
    }
}
