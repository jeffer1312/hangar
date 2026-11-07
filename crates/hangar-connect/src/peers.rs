//! Máquinas da mesma conta: descoberta pelo serviço e ligação com credencial por direção.
//!
//! A pede um aparelho em B, o dono aprova o cartão em B, e A entrega a B um aparelho que ela mesma
//! emitiu. Aqui nada conhece o `peers.json`: quem liga passa `save`, e o `hangar-server` grava.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use hangar_core::credentials;
use reqwest::header::CONTENT_TYPE;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ConnectError;
use super::config::{valid_host, valid_label};
use super::devices::{PENDING_TTL, Refusal, Store};
use super::state::{Linked, service_base};
use super::supervisor::off_thread;

/// A descoberta segura uma tela aberta: serviço fora não pode prender o Tailscale junto.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
const LINK_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_EVERY: Duration = Duration::from_secs(1);
/// Passa do prazo do cartão em B, que responde `denied`; quem vence aqui é B fora do ar até o fim.
const APPROVAL_PATIENCE: Duration = Duration::from_secs(PENDING_TTL + 60);
/// Domínio das máquinas do serviço padrão: máquina sem ligação não tem de onde tirar outro.
const DEFAULT_DOMAIN: &str = "hangar.dev.br";

/// Uma linha de `GET /v1/machines`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct AccountMachine {
    pub machine_id: String,
    pub name: String,
    pub host: String,
    pub online: bool,
}

enum Failure {
    Unreachable,
    Refused(u16),
    Invalid,
}

impl Failure {
    fn error(&self, who: &str) -> ConnectError {
        match self {
            Failure::Unreachable => ConnectError::new(StatusCode::BAD_GATEWAY, "connect_peer_unreachable", format!("{who} não respondeu")),
            Failure::Refused(status) => ConnectError::new(StatusCode::BAD_GATEWAY, "connect_peer_refused", format!("{who} recusou o pedido (HTTP {status})")),
            Failure::Invalid => ConnectError::new(StatusCode::BAD_GATEWAY, "connect_peer_invalid", format!("{who} devolveu uma resposta inválida")),
        }
    }
}

const SERVICE: &str = "o serviço do Connect";
const MACHINE: &str = "a outra máquina";

fn client(timeout: Duration) -> Result<reqwest::Client, ConnectError> {
    // Sem seguir redirecionamento: segredo e credencial iriam junto para outro endereço.
    hangar_core::net::http_client().redirect(reqwest::redirect::Policy::none()).timeout(timeout).build()
        .map_err(|_| ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_peer_unreachable", "cliente HTTP indisponível"))
}

fn post(http: &reqwest::Client, url: impl reqwest::IntoUrl, body: &Value) -> reqwest::RequestBuilder {
    http.post(url).header(CONTENT_TYPE, "application/json").body(body.to_string())
}

async fn fetch<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T, Failure> {
    let response = request.send().await.map_err(|_| Failure::Unreachable)?;
    if !response.status().is_success() {
        return Err(Failure::Refused(response.status().as_u16()));
    }
    let bytes = response.bytes().await.map_err(|_| Failure::Unreachable)?;
    serde_json::from_slice(&bytes).map_err(|_| Failure::Invalid)
}

fn service_url(linked: &Linked, path: &str) -> Result<reqwest::Url, ConnectError> {
    service_base(&linked.service_url)?.join(path).map_err(|_| Linked::invalid())
}

/// As outras máquinas da conta, com a chave desta. A própria fica de fora.
pub async fn account_machines(linked: &Linked) -> Result<Vec<AccountMachine>, ConnectError> {
    let request = client(LIST_TIMEOUT)?.get(service_url(linked, "/v1/machines")?).bearer_auth(&linked.machine_key);
    let machines: Vec<AccountMachine> = fetch(request).await.map_err(|failure| failure.error(SERVICE))?;
    Ok(machines.into_iter().filter(|m| m.machine_id != linked.machine_id && valid_label(&m.machine_id) && valid_host(&m.host)).collect())
}

/// `<id>.<conta>.<domínio>`: o host é fixo pelo nome da máquina.
pub fn expected_host(linked: &Linked, id: &str) -> Option<String> {
    linked.host.split_once('.').map(|(_, rest)| format!("{id}.{rest}"))
}

fn domain_of(host: &str) -> Option<&str> {
    host.split_once('.')?.1.split_once('.').map(|(_, domain)| domain)
}

/// `url` é endereço de máquina do Connect? `link` é o endereço desta máquina, se ela estiver ligada.
pub fn on_connect_host(link: Option<&str>, url: &str) -> bool {
    // Host com ponto final é o mesmo host para o DNS.
    let host_of = |raw: &str| reqwest::Url::parse(raw).ok().and_then(|url| url.host_str().map(|host| host.trim_end_matches('.').to_lowercase()));
    let Some(host) = host_of(url) else { return false };
    let under = |domain: &str| host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.'));
    under(DEFAULT_DOMAIN) || link.and_then(host_of).is_some_and(|own| domain_of(&own).is_some_and(under))
}

fn label(text: &str) -> String {
    text.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// As duas fontes não têm identificador em comum: o que junta é o nome da máquina.
fn same_machine(row: &Value, machine: &AccountMachine) -> bool {
    let names = [label(&machine.machine_id), label(&machine.name)];
    let hosts = row["hosts"].as_array().into_iter().flatten().filter_map(Value::as_str).filter_map(|host| host.split('.').next());
    std::iter::once(row["nome"].as_str().unwrap_or("")).chain(hosts).any(|seen| names.contains(&label(seen)))
}

/// `rows` são as linhas do Tailscale, que não mudam de endereço. Máquina vista só pela conta entra
/// se estiver no ar, como as do Tailscale.
pub fn join_discovery(mut rows: Vec<Value>, machines: Vec<AccountMachine>) -> Vec<Value> {
    for machine in machines {
        match rows.iter().position(|row| same_machine(row, &machine)) {
            Some(at) => rows[at]["connect"] = json!(machine),
            None if machine.online => rows.push(json!({"nome": machine.name, "base_url": format!("https://{}", machine.host),
                "hosts": [machine.host], "connect": machine})),
            None => {}
        }
    }
    rows
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Requesting,
    WaitingApproval,
    Linked,
    Failed,
}

/// Uma linha de `GET /api/connect/peers`. `peer` é a chave no `peers.json` depois de ligar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkRow {
    pub machine_id: String,
    pub stage: Stage,
    pub peer: Option<String>,
    pub code: Option<&'static str>,
    pub message: Option<String>,
}

/// shortcut: só em memória; reiniciar no meio perde o segredo, e o dono liga de novo. O cartão vence sozinho em B.
#[derive(Clone, Default)]
pub struct Links(Arc<Mutex<HashMap<String, LinkRow>>>);

impl Links {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, LinkRow>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn begin(&self, id: &str) -> bool {
        let mut links = self.lock();
        if links.get(id).is_some_and(|row| matches!(row.stage, Stage::Requesting | Stage::WaitingApproval)) {
            return false;
        }
        links.insert(id.to_owned(), LinkRow { machine_id: id.to_owned(), stage: Stage::Requesting, peer: None, code: None, message: None });
        true
    }

    fn waiting(&self, id: &str) {
        if let Some(row) = self.lock().get_mut(id) {
            row.stage = Stage::WaitingApproval;
        }
    }

    fn finish(&self, id: &str, outcome: &Result<Value, ConnectError>) {
        let mut links = self.lock();
        let Some(row) = links.get_mut(id) else { return };
        match outcome {
            Ok(entry) => {
                row.stage = Stage::Linked;
                row.peer = entry["id"].as_str().map(str::to_owned);
            }
            Err(error) => {
                row.stage = Stage::Failed;
                row.code = Some(error.code);
                row.message = Some(error.msg.clone());
            }
        }
    }

    pub fn list(&self) -> Vec<LinkRow> {
        let mut rows: Vec<LinkRow> = self.lock().values().cloned().collect();
        rows.sort_by(|a, b| a.machine_id.cmp(&b.machine_id));
        rows
    }
}

#[derive(Deserialize)]
struct Issued { url: String, ticket: String, signature: String }

#[derive(Deserialize)]
struct Redeemed { device_id: String }

#[derive(Deserialize)]
struct Polled { status: String, token: Option<String> }

#[derive(Deserialize)]
struct Identity { #[serde(default)] identificador: String }

/// Por onde falar com a outra máquina durante a ligação. Endereço de outro lugar levaria o segredo e,
/// depois, a credencial desta máquina. `http` só com o próprio serviço em loopback.
fn machine_base(linked: &Linked, machine: &AccountMachine, raw: &str) -> Result<String, ConnectError> {
    let url = service_base(raw).map_err(|_| Linked::invalid())?;
    let named = expected_host(linked, &machine.machine_id).as_deref() == Some(machine.host.as_str());
    let allowed = if url.scheme() == "https" {
        url.host_str() == Some(machine.host.as_str()) && url.port().is_none()
    } else {
        linked.service_url.starts_with("http://")
    };
    if !named || !allowed || url.path() != "/" || url.query().is_some() {
        return Err(Linked::invalid());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn store_failed(_: Refusal) -> ConnectError {
    ConnectError::new(StatusCode::SERVICE_UNAVAILABLE, "connect_store_failed", "os aparelhos do Connect não foram gravados")
}

/// O que a ligação precisa do servidor.
pub struct PeerLinker {
    pub linked: Linked,
    pub devices: Arc<Store>,
    /// O `CP_SERVER_ID` desta máquina: a chave dela no `peers.json` da outra.
    pub server_id: String,
    pub links: Links,
    /// Grava a entrada no `peers.json` desta máquina. Roda fora do executor assíncrono.
    pub save: Arc<dyn Fn(&Value) -> Result<(), ConnectError> + Send + Sync>,
}

impl PeerLinker {
    /// Em tarefa própria: o cartão leva minutos. `false`: esta ligação já está em curso. O andamento
    /// sai em `Links::list`.
    pub fn start(self, id: String) -> bool {
        if !self.links.begin(&id) {
            return false;
        }
        tokio::spawn(async move {
            let outcome = self.link_account_machine(&id).await;
            self.links.finish(&id, &outcome);
        });
        true
    }

    /// Devolve a entrada gravada, com a forma do `peers.json` mais o `id` que é a chave dela.
    pub async fn link_account_machine(&self, id: &str) -> Result<Value, ConnectError> {
        let machines = account_machines(&self.linked).await?;
        let machine = machines.iter().find(|machine| machine.machine_id == id)
            .ok_or_else(|| ConnectError::new(StatusCode::NOT_FOUND, "connect_peer_unknown", "essa máquina não está nesta conta"))?;
        let http = client(LINK_TIMEOUT)?;
        let secret = credentials::token();
        let asked = json!({"device_name": self.linked.machine_id, "verifier": credentials::hash(secret.as_bytes())});
        let endpoint = service_url(&self.linked, &format!("/v1/machines/{id}/devices"))?;
        let issued: Issued = fetch(post(&http, endpoint, &asked).bearer_auth(&self.linked.machine_key)).await
            .map_err(|failure| failure.error(SERVICE))?;
        let reach = machine_base(&self.linked, machine, &issued.url)?;
        let ticket = json!({"ticket": issued.ticket, "signature": issued.signature, "secret": secret});
        let redeemed: Redeemed = fetch(post(&http, format!("{reach}/api/connect/redeem"), &ticket)).await
            .map_err(|failure| failure.error(MACHINE))?;
        self.links.waiting(id);
        let token = approved_token(&http, &reach, &redeemed.device_id, &secret).await?;
        let identity: Identity = fetch(http.get(format!("{reach}/api/peers/identificador")).bearer_auth(&token)).await
            .map_err(|failure| failure.error(MACHINE))?;
        let peer = if identity.identificador.is_empty() { id.to_owned() } else { identity.identificador };
        // Outra máquina com o mesmo nome apagaria a própria entrada no `peers.json` de lá.
        if peer == self.server_id {
            return Err(ConnectError::new(StatusCode::CONFLICT, "connect_peer_same_identity", format!("a outra máquina também se chama '{peer}'")));
        }
        let (devices, name) = (self.devices.clone(), format!("máquina {id}"));
        let (device, back) = off_thread(move || devices.issue_local(&name).map_err(store_failed)).await?;
        let entry = json!({"id": peer, "base_url": format!("https://{}", machine.host), "token": token, "connect": true, "connect_return_device": device});
        // Antes de entregar: o token de B só sai uma vez, e o anúncio refaz a entrega que falhar.
        let (save, saved) = (self.save.clone(), entry.clone());
        if let Err(error) = off_thread(move || save(&saved)).await {
            let devices = self.devices.clone();
            if let Err(kept) = off_thread(move || devices.remove(&device).map_err(store_failed)).await {
                tracing::warn!(code = kept.code, "credencial emitida para outra máquina ficou sem uso: {}", kept.msg);
            }
            return Err(error);
        }
        let handed = json!({"id": self.server_id, "base_url": format!("https://{}", self.linked.host), "token": back, "connect": true});
        fetch::<Value>(post(&http, format!("{reach}/api/peers"), &handed).bearer_auth(&token)).await.map_err(|_| {
            ConnectError::new(StatusCode::BAD_GATEWAY, "connect_peer_handover_failed", "a outra máquina não recebeu a credencial de volta; anuncie de novo para repetir")
        })?;
        Ok(entry)
    }
}

async fn approved_token(http: &reqwest::Client, base: &str, device_id: &str, secret: &str) -> Result<String, ConnectError> {
    let deadline = Instant::now() + APPROVAL_PATIENCE;
    let asked = json!({"device_id": device_id, "secret": secret});
    loop {
        tokio::time::sleep(POLL_EVERY).await;
        match fetch::<Polled>(post(http, format!("{base}/api/connect/redeem/status"), &asked)).await {
            Ok(Polled { status, token: Some(token) }) if status == "approved" => return Ok(token),
            Ok(Polled { status, .. }) if status == "denied" => {
                return Err(ConnectError::new(StatusCode::FORBIDDEN, "connect_peer_denied", "o pedido foi recusado ou venceu na outra máquina"));
            }
            Ok(Polled { status, .. }) if status == "pending" => {}
            Ok(_) => return Err(Failure::Invalid.error(MACHINE)),
            // Máquina reiniciando ou túnel caído não decidem nada: quem decide é o prazo.
            Err(Failure::Unreachable) => {}
            Err(Failure::Refused(status)) if status >= 500 => {}
            Err(failure) => return Err(failure.error(MACHINE)),
        }
        if Instant::now() >= deadline {
            return Err(ConnectError::new(StatusCode::GATEWAY_TIMEOUT, "connect_peer_expired", "a outra máquina não respondeu até o fim do prazo"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked(service_url: &str) -> Linked {
        Linked { service_url: service_url.into(), account: "acme".into(), machine_id: "pc-a".into(), machine_key: String::new(),
            host: "pc-a.acme.hangar.dev.br".into(), guest_host: String::new(), preview_host: String::new(), frps_addr: String::new(),
            frps_token: String::new(), service_public_key: String::new() }
    }

    #[test]
    fn machine_address_is_fixed_by_its_name() {
        let real = linked("https://connect.hangar.dev.br");
        let b = machine("pc-b", true);
        assert_eq!(machine_base(&real, &b, "https://pc-b.acme.hangar.dev.br").unwrap(), "https://pc-b.acme.hangar.dev.br");
        for wrong in ["https://evil.test", "https://pc-c.acme.hangar.dev.br", "https://pc-b.outra.hangar.dev.br", "https://pc-b.acme.hangar.dev.br:8443",
            "https://pc-b.acme.hangar.dev.br/x", "http://127.0.0.1:9", "http://pc-b.acme.hangar.dev.br"] {
            assert!(machine_base(&real, &b, wrong).is_err(), "{wrong}");
        }
        let lying = AccountMachine { host: "evil.test".into(), ..machine("pc-b", true) };
        assert!(machine_base(&real, &lying, "https://evil.test").is_err(), "a lista do serviço não muda o host");
        assert_eq!(machine_base(&linked("http://127.0.0.1:1"), &b, "http://127.0.0.1:9").unwrap(), "http://127.0.0.1:9");
    }

    #[test]
    fn connect_hosts_are_told_by_their_domain() {
        let own = Some("https://pc-a.acme.conn.example.com");
        assert!(on_connect_host(own, "https://pc-b.acme.conn.example.com/"));
        assert!(on_connect_host(own, "https://pc-b.outra.conn.example.com"), "outra conta também é Connect");
        assert!(on_connect_host(None, "https://pc-b.acme.hangar.dev.br"), "máquina sem ligação conhece o domínio padrão");
        assert!(on_connect_host(None, "https://pc-b.acme.hangar.dev.br."), "ponto final não tira do Connect");
        assert!(on_connect_host(own, "https://pc-b.acme.conn.example.com."));
        for other in ["https://pc-b.tail.ts.net", "http://100.64.0.1:8765", "https://nothangar.dev.br", "sem-endereco"] {
            assert!(!on_connect_host(own, other), "{other}");
        }
    }

    fn machine(id: &str, online: bool) -> AccountMachine {
        AccountMachine { machine_id: id.into(), name: id.into(), host: format!("{id}.acme.hangar.dev.br"), online }
    }

    #[test]
    fn a_machine_seen_by_both_sources_appears_once_with_the_tailscale_address() {
        let rows = vec![json!({"nome": "Note Book", "base_url": "http://100.64.0.1:8765", "hosts": ["100.64.0.1", "note-book.tail.ts.net"]})];
        let joined = join_discovery(rows, vec![machine("note-book", false), machine("pc-b", true), machine("pc-c", false)]);
        assert_eq!(joined.len(), 2, "{joined:?}");
        assert_eq!(joined[0]["base_url"], "http://100.64.0.1:8765");
        assert_eq!(joined[0]["connect"]["online"], false);
        assert_eq!(joined[1]["base_url"], "https://pc-b.acme.hangar.dev.br");
    }
}
