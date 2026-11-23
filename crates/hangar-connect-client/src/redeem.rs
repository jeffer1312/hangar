//! Entrada do aparelho numa máquina da conta: bilhete do serviço, `redeem` na máquina e espera pelo token.
//! O `secret` vai direto à máquina e ao serviço só o hash dele: isso protege de um serviço honesto que vaze.
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::account::{AccountSession, ConnectMachine, auth};
use crate::client::{ConnectClient, filled, normalize_machine_url, segment};
use crate::crypto::random;
use crate::error::{Code, Error, fail};
use crate::transport::Transport;

const APPROVAL_POLL: Duration = Duration::from_secs(3);
/// O pedido pendente expira neste prazo na máquina (`PENDING_TTL`).
const APPROVAL_WINDOW: Duration = Duration::from_secs(10 * 60);

/// A máquina como o aparelho a guarda: o token sai uma vez só.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineEntry {
    /// O `machine_id` do serviço.
    pub id: String,
    pub label: String,
    /// `https://<máquina>.<conta>.hangar.dev.br`.
    pub base_url: String,
    pub token: String,
    pub device_id: String,
}

impl<T: Transport> ConnectClient<T> {
    /// Pede a entrada do aparelho na máquina. Liberado na conta, o bilhete já sai aprovado; senão (máquina
    /// antiga) espera a aprovação nela, consultando a cada 3 s por até 10 min, e `on_pending` avisa uma vez
    /// que ela foi pedida.
    ///
    /// # Errors
    ///
    /// [`Code::ConnectDeviceDenied`] (recusado na máquina), [`Code::ConnectTokenLost`] (aprovado sem token),
    /// [`Code::ConnectApprovalExpired`], [`Code::ConnectResponseInvalid`] (endereço do bilhete fora do host da
    /// máquina, resposta fora da forma), [`Code::ConnectSecureRandomUnavailable`] e os de qualquer pedido. Rede,
    /// 5xx e 429 durante a espera não encerram: a aprovação pode chegar logo depois.
    pub async fn add_machine(&self, session: &AccountSession, machine: &ConnectMachine, device_name: &str,
        install_id: Option<&str>, on_pending: impl FnOnce() + Send) -> Result<MachineEntry, Error> {
        let secret = new_secret()?;
        let verifier = sha256_hex(secret.as_bytes());
        let path = format!("/v1/machines/{}/devices", segment(&machine.machine_id));
        let ticket = self.post(&session.service_url, &path, json!({"device_name": device_name, "verifier": verifier}), auth(session)).await?;
        let url = machine_url(&ticket, &machine.host)?;
        let mut body = json!({
            "ticket": ticket.get("ticket"), "signature": ticket.get("signature"), "secret": secret, "device_name": device_name,
        });
        if let Some(install_id) = install_id.filter(|id| !id.is_empty()) {
            body["install_id"] = json!(install_id);
        }
        let redeemed = self.post(&url, "/api/connect/redeem", body, Vec::new()).await?;
        let device_id = filled(&redeemed, "device_id").ok_or_else(|| fail(Code::ConnectResponseInvalid))?.to_owned();
        let token = self.wait_for_approval(&url, &device_id, &secret, on_pending).await?;
        Ok(MachineEntry { id: machine.machine_id.clone(), label: machine.name.clone(), base_url: url, token, device_id })
    }

    async fn wait_for_approval(&self, base: &str, device_id: &str, secret: &str, on_pending: impl FnOnce() + Send) -> Result<String, Error> {
        let deadline = self.transport.now() + APPROVAL_WINDOW;
        let mut on_pending = Some(on_pending);
        // A primeira consulta sai na hora: bilhete pré-aprovado pela conta já entrega o token, sem espera nem aviso.
        let mut polls = 0u32;
        while self.transport.now() < deadline {
            if polls == 1 && let Some(notify) = on_pending.take() {
                notify();
            }
            if polls > 0 {
                self.transport.sleep(APPROVAL_POLL).await;
            }
            polls += 1;
            let answer = match self.post(base, "/api/connect/redeem/status", json!({"device_id": device_id, "secret": secret}), Vec::new()).await {
                Ok(answer) => answer,
                Err(error) if retry_later(&error) => continue,
                Err(error) => return Err(error),
            };
            match answer.get("status").and_then(Value::as_str) {
                Some("pending") => {}
                Some("denied") => return Err(fail(Code::ConnectDeviceDenied)),
                // O token sai uma vez só: aprovado sem ele é entrega perdida, e esperar não o traz de volta.
                Some("approved") => return filled(&answer, "token").map(str::to_owned).ok_or_else(|| fail(Code::ConnectTokenLost)),
                _ => return Err(fail(Code::ConnectResponseInvalid)),
            }
        }
        Err(fail(Code::ConnectApprovalExpired))
    }
}

/// Rede ou máquina instável não é recusa.
fn retry_later(error: &Error) -> bool {
    error.code() == Code::NetworkFailed || error.status().is_some_and(|status| status >= 500 || status == 429)
}

fn machine_url(ticket: &Value, host: &str) -> Result<String, Error> {
    filled(ticket, "url").and_then(normalize_machine_url)
        .filter(|url| url.starts_with("https:") && url::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_owned)).as_deref() == Some(host))
        .ok_or_else(|| fail(Code::ConnectResponseInvalid))
}

// Sem gerador seguro falha: o serviço, que tem o hash, conseguiria achar um segredo fraco.
fn new_secret() -> Result<String, Error> {
    let bytes = random::<32>().map_err(|_| fail(Code::ConnectSecureRandomUnavailable))?;
    Ok(hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
