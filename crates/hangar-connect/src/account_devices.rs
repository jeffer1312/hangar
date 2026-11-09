//! Aparelhos da conta no serviço do Connect (`/v1/devices`), falados com a chave desta máquina.
//!
//! Quem libera e revoga é a conta: aqui só se lê, se repassa o pedido e se aplica a revogação aos aparelhos locais.
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::ConnectError;
use super::devices::Store;
use super::state::{Linked, service_base};
use super::supervisor::off_thread;

/// Pedido da tela e leitura periódica: serviço fora não segura nenhum dos dois.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Lista da conta maior que isso não é lida, nem em parte: corte no meio deixaria revogado de fora.
const MAX_BODY: usize = 1024 * 1024;

/// Uma linha de `GET /v1/devices`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct AccountDevice {
    pub device_id: String,
    pub name: String,
    /// `pending`, `approved` ou `revoked`. Texto: um estado novo no serviço não derruba a lista toda.
    pub status: String,
    /// `machine` (PC da conta) ou `device`; serviço antigo não manda e todo mundo é aparelho.
    #[serde(default = "default_kind")]
    pub kind: String,
    pub created_at: f64,
    #[serde(default)]
    pub approved_at: Option<f64>,
    #[serde(default)]
    pub revoked_at: Option<f64>,
}

fn default_kind() -> String {
    "device".to_owned()
}

/// O id vira pedaço do caminho no serviço.
pub fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn invalid_id() -> ConnectError {
    ConnectError::new(StatusCode::BAD_REQUEST, "connect_account_device_invalid", "aparelho inválido")
}

fn unreachable() -> ConnectError {
    ConnectError::new(StatusCode::BAD_GATEWAY, "connect_service_unreachable", "o serviço do Connect não respondeu")
}

/// `reason` vem do corpo do serviço: o 403 de PC ainda pendente na conta não é recusa de aparelho.
fn refused(status: u16, reason: Option<&str>) -> ConnectError {
    match status {
        401 => ConnectError::new(StatusCode::CONFLICT, "connect_account_machine_unknown", "a conta não reconhece este computador: ligue-o de novo ao Connect"),
        403 if reason == Some("device_pending") => ConnectError::new(StatusCode::FORBIDDEN, "connect_account_machine_pending",
            "este computador espera liberação: libere-o em \"Aparelhos da conta\" num aparelho já liberado"),
        403 =>ConnectError::new(StatusCode::FORBIDDEN, "connect_account_device_refused", "o serviço do Connect recusou o pedido para esse aparelho"),
        404 => ConnectError::new(StatusCode::NOT_FOUND, "connect_account_device_unknown", "esse aparelho não está na conta"),
        409 => ConnectError::new(StatusCode::CONFLICT, "connect_account_device_revoked", "esse aparelho foi revogado na conta"),
        429 => ConnectError::new(StatusCode::TOO_MANY_REQUESTS, "connect_service_busy", "o serviço do Connect pediu para esperar; tente mais tarde"),
        _ => ConnectError::new(StatusCode::BAD_GATEWAY, "connect_service_failed", format!("o serviço do Connect falhou (HTTP {status})")),
    }
}

async fn call(linked: &Linked, method: reqwest::Method, path: &[&str], body: Option<serde_json::Value>) -> Result<Vec<u8>, ConnectError> {
    let mut url = service_base(&linked.service_url)?;
    url.path_segments_mut().map_err(|_| Linked::invalid())?.pop_if_empty().extend(path);
    // Sem seguir redirecionamento: a chave da máquina iria junto para outro endereço.
    let client = hangar_core::net::http_client().redirect(reqwest::redirect::Policy::none()).timeout(TIMEOUT).build()
        .map_err(|_| unreachable())?;
    let mut request = client.request(method, url).bearer_auth(&linked.machine_key);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let mut response = request.send().await.map_err(|_| unreachable())?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unreachable())? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(Linked::invalid());
        }
        body.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let reason = serde_json::from_slice::<serde_json::Value>(&body).ok();
        return Err(refused(status.as_u16(), reason.as_ref().and_then(|body| body["reason"].as_str())));
    }
    Ok(body)
}

pub async fn list(linked: &Linked) -> Result<Vec<AccountDevice>, ConnectError> {
    let bytes = call(linked, reqwest::Method::GET, &["v1", "devices"], None).await?;
    serde_json::from_slice(&bytes).map_err(|_| Linked::invalid())
}

pub async fn approve(linked: &Linked, id: &str) -> Result<(), ConnectError> {
    if !valid_id(id) { return Err(invalid_id()); }
    call(linked, reqwest::Method::POST, &["v1", "devices", id, "approve"], None).await.map(|_| ())
}

pub async fn revoke(linked: &Linked, id: &str) -> Result<(), ConnectError> {
    if !valid_id(id) { return Err(invalid_id()); }
    call(linked, reqwest::Method::DELETE, &["v1", "devices", id], None).await.map(|_| ())
}

/// Migração: o serviço só libera se o segredo for o do aparelho.
pub async fn vouch(linked: &Linked, id: &str, secret: &str) -> Result<(), ConnectError> {
    if !valid_id(id) { return Err(invalid_id()); }
    call(linked, reqwest::Method::POST, &["v1", "devices", id, "vouch"], Some(json!({"device_secret": secret}))).await.map(|_| ())
}

/// Aplica a lista inteira de revogados a cada leitura: bilhete pré-aprovado resgatado depois da leitura
/// anterior nasce aprovado mesmo com o aparelho já revogado na conta.
pub async fn apply_revocations(linked: &Linked, devices: &Arc<Store>) -> Result<(), ConnectError> {
    let revoked: Vec<String> = list(linked).await?.into_iter().filter(|d| d.status == "revoked").map(|d| d.device_id).collect();
    if revoked.is_empty() {
        return Ok(());
    }
    let devices = devices.clone();
    off_thread(move || devices.revoke_account_devices(&revoked).map_err(|_| {
        ConnectError::new(StatusCode::SERVICE_UNAVAILABLE, "connect_store_failed", "os aparelhos do Connect não foram gravados")
    })).await
}
