//! O Connect como caminho a mais das máquinas que já estão no `peers.json`. Corre por evento (Connect
//! ligado aqui, lista da conta lida, máquina gravada pelo dono), nunca por varredura periódica.
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use reqwest::Method;
use serde_json::{Value, json};

use super::Service;
use super::store::Book;
use crate::connect::peers::{AccountMachine, account_machines, on_connect_host};
use crate::groups::peers::{PeerBook, PeerClient, PeerError};
use crate::routes::AppState;

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
    let (store, link) = (service.store.clone(), format!("https://{}", linked.host));
    let ids = tokio::task::spawn_blocking(move || candidates(&store.tolerant(), Some(&link))).await.unwrap_or_default();
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
        let (st, service, machines) = (st.clone(), service.clone(), machines.clone());
        tokio::spawn(async move {
            let _slot = slot;
            if let Err((code, detail)) = enroll(&service, &id, &machines).await {
                tracing::warn!(code = "connect.peer_route_failed", reason = code, peer = %id, "o Connect não virou caminho de '{id}': {detail}");
                report(&st, &id, code, "o Connect não virou caminho da máquina");
            }
        });
    }
}

/// Entradas salvas pelo dono, ativas, ainda sem o Connect e sem recusa. Com a marca `connect` ou endereço já
/// no Connect não há caminho a somar.
fn candidates(book: &Book, link: Option<&str>) -> Vec<String> {
    let filled = |value: &Value| value.as_str().is_some_and(|text| !text.is_empty());
    book.into_iter()
        .filter(|(_, entry)| entry.is_object() && entry["enabled"] != false && entry["connect"] != true
            && entry["connect_refused"] != true && entry.get("connect_url").is_none() && filled(&entry["token"])
            && entry["base_url"].as_str().is_some_and(|url| !url.is_empty() && !on_connect_host(link, url)))
        .map(|(id, _)| id.clone())
        .collect()
}

/// Uma emissão por máquina e por arquivo: dois eventos juntos emitiriam duas credenciais lá.
struct Slot((PathBuf, String));

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

async fn enroll(service: &Service, id: &str, machines: &[AccountMachine]) -> Result<(), Failure> {
    // Sem o Connect: as duas chamadas levam a senha do dono, que só vai pelo endereço salvo.
    let client = PeerClient::new(PeerBook::new(Some(service.store.path.clone())));
    let failed = |e: PeerError| (code_of(&e), e.text(id));
    let state = client.call(id, Method::GET, "/api/connect", None).await.map_err(failed)?.unwrap_or_default();
    // Fora do Connect lá não é falha: o próximo evento confere de novo.
    let Some(host) = state["host"].as_str().filter(|host| !host.is_empty()) else { return Ok(()) };
    // O endereço gravado é o da lista da conta, nunca o que a outra máquina disse.
    let Some(machine) = machines.iter().find(|machine| machine.host.eq_ignore_ascii_case(host)) else {
        return Err(("connect_peer_host_outside_account", format!("'{id}' diz estar no Connect em {host}, que não é desta conta")));
    };
    let url = format!("https://{}", machine.host);
    let body = json!({"device_name": format!("máquina {}", service.identity())});
    let issued = client.call(id, Method::POST, "/api/connect/devices/self", Some(&body)).await.map_err(failed)?.unwrap_or_default();
    let (Some(device), Some(token)) = (issued["device_id"].as_str(), issued["token"].as_str()) else {
        return Err(("connect_peer_invalid_reply", format!("'{id}' não devolveu a credencial de aparelho")));
    };
    let (store, key, device_id, token) = (service.store.clone(), id.to_owned(), device.to_owned(), token.to_owned());
    let saved = tokio::task::spawn_blocking(move || store.set_connect_route(&key, &url, &token, &device_id)).await;
    let failure = match saved {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(e)) => (e.code, e.msg),
        Err(_) => ("peers_response_failed", "a gravação foi interrompida".to_owned()),
    };
    // Sem gravar aqui (entrada apagada ou recusada no meio), a credencial emitida lá ficaria valendo sem dono.
    if super::store::valid_device(device) {
        match client.call(id, Method::DELETE, &format!("/api/connect/devices/{device}"), None).await {
            Ok(_) | Err(PeerError::Refused { status: 404, .. }) => {}
            Err(e) => tracing::warn!(code = "connect.peer_revoke_failed", peer = id, device = %device, "o aparelho do Connect ficou valendo na outra máquina: {}", e.text(id)),
        }
    }
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
        let client = PeerClient::new(PeerBook::new(Some(path.clone()))).with_connect_route();
        client.call("note", Method::GET, "/x", None).await.unwrap();
        let mut rows = super::super::store::Store::new(path).list();
        super::super::with_routes(&client, &mut rows);
        assert_eq!(rows[0]["id"], "casa");
        assert!(rows[0].get("route").is_none(), "sem chamada desde a subida, o caminho fica de fora");
        assert_eq!(rows[1]["route"], "connect");
        assert!(rows[1].get("connect_token").is_none() && !rows.to_string().contains("aparelho"), "a credencial não sai na lista");
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
